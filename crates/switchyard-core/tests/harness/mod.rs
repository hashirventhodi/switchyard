//! Seam-1 test harness: drive the core only through its command/event stream.
//!
//! `Harness` owns a core and a throwaway data directory; `send` dispatches one command
//! and returns the events the core emitted while handling it. `relaunch` drops the core
//! and starts a new one on the same data directory, as quitting and reopening the app
//! would. `TempRepo` builds real git repositories in temporary directories.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{channel, Receiver};

use switchyard_core::Core;
use switchyard_schema::{Command, Event, Repository};
use tempfile::TempDir;

pub struct Harness {
    core: Core,
    events: Receiver<Event>,
    data_dir: TempDir,
}

impl Harness {
    pub fn launch() -> Self {
        Self::launch_in(TempDir::new().unwrap())
    }

    fn launch_in(data_dir: TempDir) -> Self {
        let (tx, events) = channel();
        let core = Core::open(data_dir.path(), tx).expect("core opens");
        Harness { core, events, data_dir }
    }

    /// Quit and reopen: a new core on the same data directory.
    pub fn relaunch(self) -> Self {
        let Harness { core, data_dir, .. } = self;
        drop(core);
        Self::launch_in(data_dir)
    }

    pub fn send(&mut self, command: Command) -> Vec<Event> {
        self.core.dispatch(command);
        self.events.try_iter().collect()
    }

    pub fn add_repository(&mut self, path: &Path) -> Vec<Event> {
        self.send(Command::AddRepository { path: path.to_string_lossy().into_owned() })
    }
}

/// The single Repository announced in `events`.
pub fn added_repository(events: &[Event]) -> Repository {
    let repos: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::RepositoryAdded { repository } => Some(repository.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(repos.len(), 1, "expected exactly one RepositoryAdded in {events:#?}");
    repos.into_iter().next().unwrap()
}

/// A real git repository on `main` with one commit, in a temporary directory.
pub struct TempRepo {
    _root: TempDir,
    pub path: PathBuf,
}

impl TempRepo {
    pub fn new(name: &str) -> Self {
        let root = TempDir::new().unwrap();
        let path = root.path().canonicalize().unwrap().join(name);
        std::fs::create_dir(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("README"), "hello\n").unwrap();
        git(&path, &["add", "README"]);
        git(&path, &["commit", "-q", "-m", "initial"]);
        TempRepo { _root: root, path }
    }

    /// `git worktree add -b <branch>` into a sibling directory; returns its path.
    pub fn add_worktree(&self, branch: &str) -> PathBuf {
        let dir = self.path.parent().unwrap().join(branch.replace('/', "-"));
        git(&self.path, &["worktree", "add", "-q", "-b", branch, dir.to_str().unwrap()]);
        dir
    }
}

pub fn git(dir: &Path, args: &[&str]) {
    let out = Process::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}
