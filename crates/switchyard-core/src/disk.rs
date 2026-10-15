//! What the core does when a Worktree changes on disk (ADR-0008). Every known Worktree is
//! watched through the [`crate::watcher::Watcher`] in the Runtime; the paths it reports are
//! sorted into kinds of change, and [`worktree_changed`] acts on each kind and then pushes
//! the Worktree's facts. The end of an agent turn comes through [`worktree_changed`] too.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use switchyard_schema::{ChangeKind, Event, Worktree};

use crate::store::StoredRepository;
use crate::watcher::{ChangeSink, Watch};
use crate::{context, git, mcp, policy, repo_policy, sessions, tasks, worktrees, Inner};

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
    let mut primary = false;
    // A primary worktree's `.git` is inside it already.
    if let Some(common) = git::common_dir(worktree) {
        let inside = worktree.canonicalize().map(|w| common.starts_with(w)).unwrap_or(false);
        if !inside {
            roots.push(common);
        }
        primary = inside;
    }
    roots.extend(memory_root(inner, worktree));
    // The Repository's own configuration is watched once, with its primary worktree.
    if primary {
        roots.extend(tasks_root(inner, worktree));
    }
    let watch = Watch { worktree: worktree.to_path_buf(), roots };
    // A watcher that cannot start leaves the Worktree to the other refresh triggers.
    let _ = inner.runtime.watcher.watch(watch, sink(inner, worktree));
}

/// Stop watching the Worktree at `worktree`; it is no longer one of its Repository's.
pub(crate) fn unwatch(inner: &mut Inner, worktree: &Path) {
    inner.runtime.watcher.unwatch(worktree);
    for list in inner.listed.values_mut() {
        list.retain(|w| Path::new(&w.path) != worktree);
    }
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
    let mut kinds = classify(worktree, paths);
    kinds.config |= touches_config(inner, worktree, paths);
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
    signal(inner, worktree_path, kinds);
}

/// Tell whichever view shows heavy data for the Worktree (Changes, History, Context) to
/// re-read it. Diffs are computed only when a view asks.
fn signal(inner: &Inner, worktree_path: &str, kinds: Kinds) {
    let list = [(kinds.files, ChangeKind::Files), (kinds.git, ChangeKind::Git), (kinds.config, ChangeKind::Config)];
    let kinds = list.into_iter().filter(|(on, _)| *on).map(|(_, kind)| kind).collect();
    inner.emit(Event::WorktreeChanged { worktree_path: worktree_path.into(), kinds });
}

/// Working-tree files changed. Nothing beyond the facts yet.
fn files_changed(_inner: &mut Inner, _worktree_path: &str) {}

/// Git's records changed (a commit, a fetch, a checkout, a worktree added, removed or
/// renamed in a shell): re-list the Repository's worktrees from git.
fn git_changed(inner: &mut Inner, worktree_path: &str) {
    let owner = inner.listed.iter().find(|(_, list)| list.iter().any(|w| w.path == worktree_path));
    let Some(repository_id) = owner.map(|(id, _)| id.clone()) else { return };
    let Some(repo) = worktrees::repository(inner, &repository_id) else { return };
    relist(inner, &repo);
}

/// Remember an announced Repository's worktrees as git lists them now.
pub(crate) fn remember_repository(inner: &mut Inner, repo: &StoredRepository) {
    if let Some(list) = listed(repo) {
        inner.listed.insert(repo.id.clone(), list);
    }
}

/// Remember one worktree Switchyard added, so a re-list does not announce it again.
pub(crate) fn remember_added(inner: &mut Inner, repository_id: &str, worktree: &Worktree) {
    let list = inner.listed.entry(repository_id.to_string()).or_default();
    list.retain(|w| w.path != worktree.path);
    list.push(worktree.clone());
}

/// Compare git's worktrees with the ones last announced: announce one that is new or whose
/// branch changed (and watch a new one); one git no longer lists is forgotten, its session
/// stopped and archived.
fn relist(inner: &mut Inner, repo: &StoredRepository) {
    // A Repository git cannot read is left as it was.
    let Some(now) = listed(repo) else { return };
    let before = inner.listed.insert(repo.id.clone(), now.clone()).unwrap_or_default();
    for gone in before.iter().filter(|w| !now.iter().any(|n| n.path == w.path)) {
        sessions::archive_gone(inner, &gone.path);
        worktrees::forget(inner, &repo.id, &gone.path);
    }
    for worktree in &now {
        if before.contains(worktree) {
            continue;
        }
        let new = !before.iter().any(|w| w.path == worktree.path);
        inner.emit(Event::WorktreeAdded { repository_id: repo.id.clone(), worktree: worktree.clone() });
        if new {
            watch(inner, Path::new(&worktree.path));
            worktrees::refresh_one(inner, &worktree.path);
        }
    }
}

/// The Repository's worktrees as git lists them, primary first; `None` when git cannot.
fn listed(repo: &StoredRepository) -> Option<Vec<Worktree>> {
    let list = git::worktrees(Path::new(&repo.path)).ok().filter(|l| !l.is_empty())?;
    Some(
        list.into_iter()
            .map(|wt| Worktree { path: wt.path.to_string_lossy().into_owned(), branch: wt.branch, primary: wt.primary })
            .collect(),
    )
}

/// Configuration files changed: re-read them and push what the in-app commands push.
fn config_changed(inner: &mut Inner, worktree_path: &str) {
    let Some((repo, _)) = inner.find_worktree(worktree_path) else { return };
    tasks::read(inner, &repo.id);
    // The policy is re-hashed as it is resolved: an expanded privilege is a request again.
    policy::emit(inner, &repo);
    mcp::announce(inner, &repo);
    context::read_context(inner, worktree_path);
}

/// Whether any of `paths` is configuration the Worktree at `worktree` is watched for: its
/// instruction files, Claude's memory for it, and the Repository's policy, MCP servers and
/// local tasks.
fn touches_config(inner: &Inner, worktree: &Path, paths: &[PathBuf]) -> bool {
    let memory = context::memory_dir(&inner.runtime.claude_home, &worktree.to_string_lossy());
    let tasks = inner.find_worktree(&worktree.to_string_lossy()).map(|(repo, _)| tasks::local_file(inner, &repo));
    paths.iter().any(|path| {
        let path = if path.is_relative() { worktree.join(path) } else { path.clone() };
        let in_worktree = relative_to(&path, worktree);
        in_worktree.is_some_and(|rel| context::is_instructions(rel) || is_repository_config(rel))
            || relative_to(&path, &memory).is_some()
            || tasks.as_deref().is_some_and(|file| relative_to(&path, file).is_some())
    })
}

/// The local tasks file lives in Switchyard's data directory, outside the Repository: watch
/// its directory (made here if no task was ever added, so the first one is seen).
fn tasks_root(inner: &Inner, primary: &Path) -> Option<PathBuf> {
    let (repo, _) = inner.find_worktree(&primary.to_string_lossy())?;
    let dir = tasks::local_file(inner, &repo).parent()?.to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The Repository's committed configuration (read from its primary worktree).
fn is_repository_config(rel: &Path) -> bool {
    let policy = Path::new(repo_policy::POLICY_FILE);
    rel == policy || Some(rel) == policy.parent() || rel == Path::new(mcp::MCP_FILE)
}

/// Claude's memory for the Worktree lives outside it, under `claude_home`: watch the memory
/// directory, or the project directory it will appear in, whichever exists (Claude makes
/// them; Switchyard only reads them).
fn memory_root(inner: &Inner, worktree: &Path) -> Option<PathBuf> {
    let memory = context::memory_dir(&inner.runtime.claude_home, &worktree.to_string_lossy());
    let project = memory.parent().map(Path::to_path_buf);
    std::iter::once(memory).chain(project).find(|dir| dir.is_dir())
}

/// `path` relative to `base`, taking `base` as given or with its links resolved (the OS
/// reports resolved paths).
fn relative_to<'a>(path: &'a Path, base: &Path) -> Option<&'a Path> {
    path.strip_prefix(base).ok().or_else(|| base.canonicalize().ok().and_then(|b| path.strip_prefix(b).ok()))
}

/// Sort reported paths into kinds of change. Under the Repository's `.git` only HEAD, the
/// index, refs, packed refs, the HEAD log and `worktrees/` count; in the working tree only
/// paths git does not ignore count.
fn classify(worktree: &Path, paths: &[PathBuf]) -> Kinds {
    let mut kinds = Kinds::default();
    // A Worktree whose folder is gone was removed (in a shell, say): what git lists changed.
    if !worktree.exists() {
        kinds.git = true;
        return kinds;
    }
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
