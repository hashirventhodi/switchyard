//! What the core does when a Worktree changes on disk (ADR-0008). Every known Worktree is
//! watched through the [`crate::watcher::Watcher`] in the Runtime; the paths it reports are
//! sorted into kinds of change, and [`worktree_changed`] acts on each kind and then pushes
//! the Worktree's facts. The end of an agent turn comes through [`worktree_changed`] too.
//! A Worktree whose watcher cannot run is *unwatched*: the core says so, re-derives it
//! through [`worktree_changed`] on every [`fallback_tick`] and tries to watch it again.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use switchyard_schema::{ChangeKind, Event, Worktree};

use crate::store::StoredRepository;
use crate::watcher::{ChangeSink, Watch, WatchReport};
use crate::{context, git, mcp, policy, repo_policy, review, sessions, tasks, worktrees, Inner};

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
pub(crate) fn watch_repository(inner: &mut Inner, dir: &Path) {
    let Ok(list) = git::worktrees(dir) else { return };
    for wt in list {
        watch(inner, &wt.path);
    }
}

/// Start (or restart) watching the Worktree at `worktree`: its working tree and its
/// Repository's `.git`. A watch that cannot start leaves the Worktree unwatched, and says
/// so even when it already was (attach replays it this way); one that starts clears that.
pub(crate) fn watch(inner: &mut Inner, worktree: &Path) {
    match start(inner, worktree) {
        Ok(()) => watched(inner, worktree),
        Err(reason) => {
            let worktree_path = worktree.to_string_lossy().into_owned();
            inner.unwatched.insert(worktree_path.clone(), reason.clone());
            inner.emit(Event::WorktreeUnwatched { worktree_path, reason });
        }
    }
}

fn start(inner: &mut Inner, worktree: &Path) -> Result<(), String> {
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
    let memory = memory_root(inner, worktree);
    roots.extend(memory.clone());
    // The Repository's own configuration is watched once, with its primary worktree.
    if primary {
        roots.extend(tasks_root(inner, worktree));
    }
    let watch = Watch { worktree: worktree.to_path_buf(), roots };
    inner.runtime.watcher.watch(watch, sink(inner, worktree))?;
    let worktree_path = worktree.to_string_lossy().into_owned();
    match memory {
        Some(memory) => inner.watched_memory.insert(worktree_path, memory),
        None => inner.watched_memory.remove(&worktree_path),
    };
    Ok(())
}

/// Claude makes a Worktree's memory directory (and the project directory it sits in) when
/// it first writes there, which nothing watched may cover yet: once it is there, watch the
/// Worktree again so it is. A Worktree whose watcher cannot run is left to the fallback.
pub(crate) fn watch_new_memory(inner: &mut Inner, worktree_path: &str) {
    let announced = inner.listed.values().any(|list| list.iter().any(|w| w.path == worktree_path));
    if inner.unwatched.contains_key(worktree_path) || !announced {
        return;
    }
    let worktree = Path::new(worktree_path);
    let now = memory_root(inner, worktree);
    if now.is_some() && now.as_ref() != inner.watched_memory.get(worktree_path) {
        watch(inner, worktree);
    }
}

/// The Worktree at `worktree` is watched: if it was not, live updates resume.
fn watched(inner: &mut Inner, worktree: &Path) {
    let worktree_path = worktree.to_string_lossy().into_owned();
    if inner.unwatched.remove(&worktree_path).is_some() {
        inner.emit(Event::WorktreeWatched { worktree_path });
    }
}

/// Stop watching the Worktree at `worktree`; it is no longer one of its Repository's.
pub(crate) fn unwatch(inner: &mut Inner, worktree: &Path) {
    inner.runtime.watcher.unwatch(worktree);
    inner.unwatched.remove(&*worktree.to_string_lossy());
    inner.watched_memory.remove(&*worktree.to_string_lossy());
    for list in inner.listed.values_mut() {
        list.retain(|w| Path::new(&w.path) != worktree);
    }
}

/// The watch on `worktree` failed after it started: fall back until it can be watched again.
fn failed(inner: &mut Inner, worktree: &Path, reason: String) {
    inner.runtime.watcher.unwatch(worktree);
    let worktree_path = worktree.to_string_lossy().into_owned();
    if inner.unwatched.insert(worktree_path.clone(), reason.clone()).is_none() {
        inner.emit(Event::WorktreeUnwatched { worktree_path, reason });
    }
}

/// One beat of the slow cadence for unwatched Worktrees: try each one's watcher again, and
/// re-derive it the way a change on disk would (once more after a watch starts, to catch up
/// on what happened while nothing was watching).
pub(crate) fn fallback_tick_locking(core: &Mutex<Inner>) {
    let mut inner = core.lock().expect("core lock");
    fallback_tick(&mut inner);
    sessions::flush(&mut inner);
}

fn fallback_tick(inner: &mut Inner) {
    let unwatched: Vec<String> = inner.unwatched.keys().cloned().collect();
    for worktree_path in unwatched {
        let worktree = PathBuf::from(&worktree_path);
        match start(inner, &worktree) {
            Ok(()) => watched(inner, &worktree),
            Err(reason) => {
                inner.unwatched.insert(worktree_path.clone(), reason);
            }
        }
        worktree_changed(inner, &worktree_path, Kinds { files: true, git: true, config: true });
    }
}

/// Where the watcher's reports re-enter the core: on the watcher's thread, like agent output.
fn sink(inner: &Inner, worktree: &Path) -> ChangeSink {
    let core = inner.me.clone();
    let worktree = worktree.to_path_buf();
    Arc::new(move |report| {
        if let Some(core) = core.upgrade() {
            let mut inner = core.lock().expect("core lock");
            match report {
                WatchReport::Changed(paths) => disk_changed(&mut inner, &worktree, &paths),
                WatchReport::Failed(reason) => failed(&mut inner, &worktree, reason),
            }
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
    if kinds.files || kinds.git {
        // A file committed or reverted outside Switchyard takes its Decisions with it.
        review::prune_settled(inner, worktree_path);
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

/// Git's records changed (a commit, a fetch, a branch switch, a worktree added, removed or
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
    // Where this Worktree's own HEAD and index live in `common`: at its root for the
    // primary, under `worktrees/<name>` for a linked one.
    let own = git::git_dir(worktree).zip(common.as_deref()).and_then(|(g, c)| g.strip_prefix(c).ok().map(Path::to_path_buf));
    let canonical = worktree.canonicalize().ok();
    let mut files = Vec::new();
    for path in paths {
        let path = if path.is_relative() { worktree.join(path) } else { path.clone() };
        if let Some(rel) = common.as_deref().and_then(|c| path.strip_prefix(c).ok()) {
            kinds.git |= own.as_deref().is_some_and(|own| is_git_record(rel, own));
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

/// Whether `rel`, under the Repository's `.git`, is one of git's records for the Worktree
/// whose own git directory is `own` (relative to `.git`; empty for the primary). Refs and
/// packed refs are every Worktree's; HEAD, the index and the HEAD log only their own
/// Worktree's. Another worktree's directory under `worktrees/` counts only when it comes or
/// goes (the directory itself, or its `gitdir` file, which also moves with it), so one
/// worktree's commits do not re-derive its neighbours.
fn is_git_record(rel: &Path, own: &Path) -> bool {
    if rel == Path::new("packed-refs") || rel.starts_with("refs") {
        return true;
    }
    if let Ok(mine) = rel.strip_prefix(own) {
        if mine == Path::new("HEAD") || mine == Path::new("index") || mine == Path::new("logs/HEAD") {
            return true;
        }
    }
    let Ok(entry) = rel.strip_prefix("worktrees") else { return false };
    let parts: Vec<_> = entry.components().collect();
    parts.len() <= 1 || (parts.len() == 2 && parts[1].as_os_str() == "gitdir")
}
