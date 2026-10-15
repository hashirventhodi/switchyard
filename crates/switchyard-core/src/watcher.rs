//! How the core hears that a Worktree changed on disk (ADR-0008). A [`Watcher`] in the
//! [`crate::Runtime`] is told which directories belong to a Worktree and reports changed
//! paths under them; what a path means (working tree, git, ignored) is the core's business.
//! The app runs [`FsWatcher`] on the real filesystem; tests drive a fake by hand.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
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

/// Receives the paths that changed under a [`Watch`] since the last call (coalesced).
pub type ChangeSink = Arc<dyn Fn(Vec<PathBuf>) + Send + Sync>;

pub trait Watcher: Send + Sync {
    /// Start (or restart) watching `watch.roots`, calling `sink` with changed paths. Watching
    /// a Worktree again replaces its earlier watch.
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
    paths: Vec<PathBuf>,
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
                    Ok(batch) => {
                        let entry = pending
                            .entry(batch.worktree)
                            .or_insert_with(|| (Instant::now() + window, batch.sink.clone(), Vec::new()));
                        entry.1 = batch.sink;
                        entry.2.extend(batch.paths);
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
                        sink(paths);
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
        let mut os = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            // Errors are left to the fallback (a Worktree whose watcher fails is re-derived
            // on a slow cadence); here a bad notification is just dropped.
            if let Ok(event) = result {
                if !event.paths.is_empty() {
                    let _ = tx.send(Batch { worktree: worktree.clone(), sink: sink.clone(), paths: event.paths });
                }
            }
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
