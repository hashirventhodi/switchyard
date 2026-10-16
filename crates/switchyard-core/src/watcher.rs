//! How the core hears that a Worktree changed on disk (ADR-0008). A [`Watcher`] in the
//! [`crate::Runtime`] is told which directories belong to a Worktree and reports changed
//! paths under them; what a path means (working tree, git, ignored) is the core's business.
//! The app runs [`FsWatcher`] on the real filesystem; tests drive a fake by hand.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What to watch for one Worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    /// The Worktree's path, as git lists it: the key for [`Watcher::unwatch`].
    pub worktree: PathBuf,
    /// Directories to watch recursively: the working tree and the Repository's `.git`.
    pub roots: Vec<PathBuf>,
}

/// What a watcher reports about one [`Watch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchReport {
    /// These paths changed since the last report (coalesced).
    Changed(Vec<PathBuf>),
    /// The watch stopped working, and why; nothing more is reported for it. The core falls
    /// back to re-deriving the Worktree on a slow cadence and watches it again later.
    Failed(String),
}

/// Receives what a watcher reports about one [`Watch`].
pub type ChangeSink = Arc<dyn Fn(WatchReport) + Send + Sync>;

pub trait Watcher: Send + Sync {
    /// Start (or restart) watching `watch.roots`, calling `sink` with changed paths. Watching
    /// a Worktree again replaces its earlier watch.
    /// An `Err` says why the watch could not start.
    fn watch(&self, watch: Watch, sink: ChangeSink) -> Result<(), String>;
    /// Stop watching the Worktree at `worktree`; nothing more is reported for it.
    fn unwatch(&self, worktree: &Path);
}

/// Watches nothing: changes on disk are only seen when something else refreshes.
pub struct NoWatcher;

impl Watcher for NoWatcher {
    fn watch(&self, _watch: Watch, _sink: ChangeSink) -> Result<(), String> {
        Ok(())
    }
    fn unwatch(&self, _worktree: &Path) {}
}

/// How often a Worktree whose watcher cannot run is re-derived, and its watcher tried again.
pub const FALLBACK_EVERY: Duration = Duration::from_secs(10);

/// How long notifications for one Worktree are gathered before the core acts on them.
pub const COALESCE: Duration = Duration::from_millis(500);

/// The real filesystem watcher (`notify`): one OS watcher per Worktree, notifications
/// coalesced per Worktree over [`COALESCE`] on one delivery thread, which is the only
/// thread that calls sinks (so the OS watcher's threads never wait on the core).
///
/// Off macOS each directory is its own OS watch (inotify gives one per directory, from a
/// limited budget), so the watcher skips folders git ignores (`node_modules`, `target`):
/// it walks each root honouring `.gitignore`, watches the folders it keeps one by one, and
/// watches a folder made later once it is reported, unless it is ignored. On macOS FSEvents
/// watches a whole tree as one stream with no per-directory cost, and a stream over every
/// folder would be restarted for each new one; there the roots are watched recursively and
/// the core sets ignored paths aside when it sorts what changed.
pub struct FsWatcher {
    watches: Arc<Mutex<HashMap<PathBuf, Watched>>>,
    deliver: Mutex<Sender<Batch>>,
    skip_ignored: bool,
}

/// One Worktree's OS watcher, and the folders it watches one by one (when skipping ignored
/// ones; empty otherwise).
struct Watched {
    os: notify::RecommendedWatcher,
    folders: HashSet<PathBuf>,
}

struct Batch {
    worktree: PathBuf,
    sink: ChangeSink,
    /// What changed; `Err` when the OS watch reported an error.
    paths: Result<Vec<PathBuf>, String>,
}

impl FsWatcher {
    pub fn new() -> Self {
        Self::coalescing_over(COALESCE)
    }

    /// Coalescing over `window`, skipping folders git ignores where that saves OS watches
    /// (everywhere but macOS: see [`FsWatcher`]).
    pub fn coalescing_over(window: Duration) -> Self {
        Self::start(window, !cfg!(target_os = "macos"))
    }

    /// Coalescing over `window`, always skipping folders git ignores, macOS included.
    pub fn skipping_ignored_over(window: Duration) -> Self {
        Self::start(window, true)
    }

    fn start(window: Duration, skip_ignored: bool) -> Self {
        let (tx, rx) = channel::<Batch>();
        let watches: Arc<Mutex<HashMap<PathBuf, Watched>>> = Arc::default();
        let known = watches.clone();
        std::thread::spawn(move || {
            // Per Worktree: when to deliver, to whom, and what has changed so far.
            let mut pending: HashMap<PathBuf, (Instant, ChangeSink, Vec<PathBuf>)> = HashMap::new();
            loop {
                let next = pending.values().map(|(due, ..)| *due).min();
                let received = match next {
                    Some(due) => rx.recv_timeout(due.saturating_duration_since(Instant::now())),
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match received {
                    Ok(Batch { worktree, sink, paths: Err(reason) }) => {
                        // A failed watch reports nothing more, so what was gathered is moot.
                        pending.remove(&worktree);
                        sink(WatchReport::Failed(reason));
                    }
                    Ok(Batch { worktree, sink, paths: Ok(mut paths) }) => {
                        if skip_ignored {
                            let made = watch_new_folders(&known, &worktree, &paths);
                            paths.extend(made);
                        }
                        let entry = pending
                            .entry(worktree)
                            .or_insert_with(|| (Instant::now() + window, sink.clone(), Vec::new()));
                        entry.1 = sink;
                        entry.2.extend(paths);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                let now = Instant::now();
                let due: Vec<PathBuf> = pending.iter().filter(|(_, (at, ..))| *at <= now).map(|(k, _)| k.clone()).collect();
                for worktree in due {
                    if let Some((_, sink, mut paths)) = pending.remove(&worktree) {
                        paths.sort();
                        paths.dedup();
                        sink(WatchReport::Changed(paths));
                    }
                }
            }
        });
        FsWatcher { watches, deliver: Mutex::new(tx), skip_ignored }
    }
}

impl Default for FsWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// A walk of `root` that leaves out what git ignores there (and git's object store, which
/// the core never reads). Hidden folders are kept: `.git` and `.switchyard` are watched.
fn walk(root: &Path) -> ignore::WalkBuilder {
    let mut walk = ignore::WalkBuilder::new(root);
    walk.hidden(false).filter_entry(|entry| {
        let objects = entry.file_name() == "objects" && entry.path().parent().and_then(Path::file_name) == Some(".git".as_ref());
        !objects
    });
    walk
}

/// The folders under `root` (itself included) that the walk keeps, and the files in them.
fn kept(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut folders, mut files) = (Vec::new(), Vec::new());
    for entry in walk(root).build().flatten() {
        match entry.file_type() {
            Some(t) if t.is_dir() => folders.push(entry.into_path()),
            Some(_) => files.push(entry.into_path()),
            None => {}
        }
    }
    (folders, files)
}

/// Whether the folder at `path` is one a walk of its parent keeps (git does not ignore it).
fn admitted(path: &Path) -> bool {
    let Some(parent) = path.parent() else { return true };
    walk(parent).max_depth(Some(1)).build().flatten().any(|entry| entry.path() == path)
}

/// Watch the folders among `paths` that appeared since the Worktree at `worktree` was
/// watched and git does not ignore, with everything under them. Returns the files found in
/// them, which may have been written before their folder was watched.
fn watch_new_folders(watches: &Mutex<HashMap<PathBuf, Watched>>, worktree: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    use notify::Watcher as _;
    let mut watches = watches.lock().expect("watcher lock");
    let Some(watched) = watches.get_mut(worktree) else { return Vec::new() };
    let mut found = Vec::new();
    for path in paths {
        if watched.folders.contains(path) || !path.is_dir() || !admitted(path) {
            continue;
        }
        let (folders, files) = kept(path);
        for folder in folders {
            if !watched.folders.contains(&folder) && watched.os.watch(&folder, notify::RecursiveMode::NonRecursive).is_ok() {
                watched.folders.insert(folder);
            }
        }
        found.extend(files);
    }
    found
}

/// Whether an OS event can mean something changed. inotify also reports a file being opened,
/// read or closed unwritten (FSEvents does not); those are reads, and the core's own `git
/// status` reading the index would otherwise count as a change and feed itself.
fn changes_content(kind: &notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode};
    match kind {
        notify::EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        notify::EventKind::Access(_) => false,
        _ => true,
    }
}

impl Watcher for FsWatcher {
    fn watch(&self, watch: Watch, sink: ChangeSink) -> Result<(), String> {
        use notify::Watcher as _;
        let tx = self.deliver.lock().expect("watcher lock").clone();
        let worktree = watch.worktree.clone();
        let failed = AtomicBool::new(false);
        let mut os = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            // An error (a lost watch, an overflowed queue) fails the watch: the core falls back
            // to re-deriving the Worktree on a slow cadence. Reported once.
            let paths = match result {
                Ok(event) if event.paths.is_empty() || !changes_content(&event.kind) => return,
                Ok(event) => Ok(event.paths),
                Err(_) if failed.swap(true, Ordering::SeqCst) => return,
                Err(e) => Err(format!("the watcher for {} failed: {e}", worktree.display())),
            };
            let _ = tx.send(Batch { worktree: worktree.clone(), sink: sink.clone(), paths });
        })
        .map_err(|e| format!("cannot watch {}: {e}", watch.worktree.display()))?;
        let mut folders = HashSet::new();
        for root in &watch.roots {
            if !self.skip_ignored {
                os.watch(root, notify::RecursiveMode::Recursive)
                    .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;
                continue;
            }
            // Links resolved, as the OS reports paths, so a folder reported is found watched.
            let root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let mut paths = os.paths_mut();
            for folder in kept(&root).0 {
                paths.add(&folder, notify::RecursiveMode::NonRecursive)
                    .map_err(|e| format!("cannot watch {}: {e}", folder.display()))?;
                folders.insert(folder);
            }
            paths.commit().map_err(|e| format!("cannot watch {}: {e}", root.display()))?;
        }
        self.watches.lock().expect("watcher lock").insert(watch.worktree, Watched { os, folders });
        Ok(())
    }

    fn unwatch(&self, worktree: &Path) {
        let removed = self.watches.lock().expect("watcher lock").remove(worktree);
        drop(removed);
    }
}
