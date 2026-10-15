//! How the core hears that a Worktree changed on disk (ADR-0008). A [`Watcher`] in the
//! [`crate::Runtime`] is told which directories belong to a Worktree and reports changed
//! paths under them; what a path means (working tree, git, ignored) is the core's business.
//! The app runs [`FsWatcher`] on the real filesystem; tests drive a fake by hand.

use std::collections::HashMap;
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

/// The real filesystem watcher (`notify`): one OS watch per Worktree, notifications
/// coalesced per Worktree over [`COALESCE`] on one delivery thread, which is the only
/// thread that calls sinks (so the OS watcher's threads never wait on the core).
pub struct FsWatcher {
    watches: Mutex<HashMap<PathBuf, notify::RecommendedWatcher>>,
    deliver: Mutex<Sender<Batch>>,
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

    pub fn coalescing_over(window: Duration) -> Self {
        let (tx, rx) = channel::<Batch>();
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
                    Ok(Batch { worktree, sink, paths: Ok(paths) }) => {
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
        FsWatcher { watches: Mutex::new(HashMap::new()), deliver: Mutex::new(tx) }
    }
}

impl Default for FsWatcher {
    fn default() -> Self {
        Self::new()
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
                Ok(event) if event.paths.is_empty() => return,
                Ok(event) => Ok(event.paths),
                Err(_) if failed.swap(true, Ordering::SeqCst) => return,
                Err(e) => Err(format!("the watcher for {} failed: {e}", worktree.display())),
            };
            let _ = tx.send(Batch { worktree: worktree.clone(), sink: sink.clone(), paths });
        })
        .map_err(|e| format!("cannot watch {}: {e}", watch.worktree.display()))?;
        for root in &watch.roots {
            os.watch(root, notify::RecursiveMode::Recursive)
                .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;
        }
        self.watches.lock().expect("watcher lock").insert(watch.worktree, os);
        Ok(())
    }

    fn unwatch(&self, worktree: &Path) {
        let removed = self.watches.lock().expect("watcher lock").remove(worktree);
        drop(removed);
    }
}
