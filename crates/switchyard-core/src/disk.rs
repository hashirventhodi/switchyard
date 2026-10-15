//! What the core does when a Worktree changes on disk (ADR-0008). Every known Worktree is
//! watched through the [`crate::watcher::Watcher`] in the Runtime; the paths it reports are
//! sorted into kinds of change, and [`worktree_changed`] acts on each kind and then pushes
//! the Worktree's facts. The end of an agent turn comes through [`worktree_changed`] too.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::watcher::{ChangeSink, Watch};
use crate::{git, sessions, worktrees, Inner};

/// What kinds of thing changed in a Worktree. Each kind has its own branch in
/// [`worktree_changed`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Kinds {
    /// Files in the working tree that git does not ignore.
    pub files: bool,
    /// What git records: HEAD, the index, refs, packed refs, the HEAD log, worktrees.
    pub git: bool,
    /// The Repository's configuration files.
    pub config: bool,
}

impl Kinds {
    pub(crate) fn any(self) -> bool {
        self.files || self.git || self.config
    }
}

/// Start watching every worktree git lists for the repository at `dir`.
pub(crate) fn watch_repository(inner: &Inner, dir: &Path) {
    let Ok(list) = git::worktrees(dir) else { return };
    for wt in list {
        watch(inner, &wt.path);
    }
}

/// Start (or restart) watching the Worktree at `worktree`: its working tree and its
/// Repository's `.git`.
pub(crate) fn watch(inner: &Inner, worktree: &Path) {
    let mut roots = vec![worktree.to_path_buf()];
    // A primary worktree's `.git` is inside it already.
    if let Some(common) = git::common_dir(worktree) {
        let inside = worktree.canonicalize().map(|w| common.starts_with(w)).unwrap_or(false);
        if !inside {
            roots.push(common);
        }
    }
    let watch = Watch { worktree: worktree.to_path_buf(), roots };
    // A watcher that cannot start leaves the Worktree to the other refresh triggers.
    let _ = inner.runtime.watcher.watch(watch, sink(inner, worktree));
}

pub(crate) fn unwatch(inner: &Inner, worktree: &Path) {
    inner.runtime.watcher.unwatch(worktree);
}

/// Where the watcher's reports re-enter the core: on the watcher's thread, like agent output.
fn sink(inner: &Inner, worktree: &Path) -> ChangeSink {
    let core = inner.me.clone();
    let worktree = worktree.to_path_buf();
    Arc::new(move |paths| {
        if let Some(core) = core.upgrade() {
            let mut inner = core.lock().expect("core lock");
            disk_changed(&mut inner, &worktree, &paths);
            sessions::flush(&mut inner);
        }
    })
}

/// The watcher reports `paths` changed under the Worktree at `worktree` (coalesced). Paths
/// may be absolute or relative to the Worktree.
pub(crate) fn disk_changed(inner: &mut Inner, worktree: &Path, paths: &[PathBuf]) {
    let kinds = classify(worktree, paths);
    worktree_changed(inner, &worktree.to_string_lossy(), kinds);
}

/// Act on a change to the Worktree at `worktree_path`: one branch per kind, then its facts,
/// which every change can move.
pub(crate) fn worktree_changed(inner: &mut Inner, worktree_path: &str, kinds: Kinds) {
    if !kinds.any() {
        return;
    }
    if kinds.files {
        files_changed(inner, worktree_path);
    }
    if kinds.git {
        git_changed(inner, worktree_path);
    }
    if kinds.config {
        config_changed(inner, worktree_path);
    }
    worktrees::refresh_one(inner, worktree_path);
}

/// Working-tree files changed. Nothing beyond the facts yet.
fn files_changed(_inner: &mut Inner, _worktree_path: &str) {}

/// Git's records changed (a commit, a fetch, a checkout). Nothing beyond the facts yet.
fn git_changed(_inner: &mut Inner, _worktree_path: &str) {}

/// Configuration files changed. Nothing beyond the facts yet.
fn config_changed(_inner: &mut Inner, _worktree_path: &str) {}

/// Sort reported paths into kinds of change. Under the Repository's `.git` only HEAD, the
/// index, refs, packed refs, the HEAD log and `worktrees/` count; in the working tree only
/// paths git does not ignore count.
fn classify(worktree: &Path, paths: &[PathBuf]) -> Kinds {
    let mut kinds = Kinds::default();
    let common = git::common_dir(worktree);
    let canonical = worktree.canonicalize().ok();
    let mut files = Vec::new();
    for path in paths {
        let path = if path.is_relative() { worktree.join(path) } else { path.clone() };
        if let Some(rel) = common.as_deref().and_then(|c| path.strip_prefix(c).ok()) {
            kinds.git |= is_git_record(rel);
            continue;
        }
        let rel = path.strip_prefix(worktree).ok().or_else(|| canonical.as_deref().and_then(|c| path.strip_prefix(c).ok()));
        // The Worktree's own folder, and a linked worktree's `.git` file, are not its files.
        match rel {
            Some(rel) if !rel.as_os_str().is_empty() && !rel.starts_with(".git") => files.push(rel.to_path_buf()),
            _ => {}
        }
    }
    if !files.is_empty() {
        // A path git cannot judge counts as a change: better a refresh too many.
        let ignored = git::ignored(worktree, &files).unwrap_or_default();
        kinds.files = files.iter().any(|f| !ignored.contains(f));
    }
    kinds
}

fn is_git_record(rel: &Path) -> bool {
    rel == Path::new("HEAD")
        || rel == Path::new("index")
        || rel == Path::new("packed-refs")
        || rel == Path::new("logs/HEAD")
        || rel.starts_with("refs")
        || rel.starts_with("worktrees")
}
