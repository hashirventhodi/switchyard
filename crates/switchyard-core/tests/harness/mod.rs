//! Seam-1 test harness: drive the core only through its command/event stream.
//!
//! `Harness` owns a core and a throwaway data directory; `send` dispatches one command
//! and returns the events the core emitted while handling it. `relaunch` drops the core
//! and starts a new one on the same data directory, as quitting and reopening the app
//! would. `TempRepo` builds real git repositories in temporary directories.
//!
//! Agents are fake: `harness.agents` records every launch and every protocol command the
//! core sends, and `harness.agent_says(n, event)` delivers a protocol event from the n-th
//! launched agent (0-based), returning the core events it caused. `agent_replays` feeds a
//! recorded fixture's expected events, so core tests run on recorded sessions.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use adapter_protocol::Event as AgentEvent;
use switchyard_core::agents::{AgentHandle, AgentLauncher, AgentOutput, AgentSink, LaunchSpec};
use switchyard_core::{Core, Runtime};
use switchyard_schema::{AgentKind, Command, Event, PermissionMode, Repository};
use tempfile::TempDir;

/// The fixed wall clock every harness core sees.
pub const NOW_MS: u64 = 1_760_000_000_000;

pub struct Harness {
    core: Core,
    events: Receiver<Event>,
    data_dir: TempDir,
    pub agents: FakeAgents,
}

impl Harness {
    pub fn launch() -> Self {
        Self::launch_in(TempDir::new().unwrap(), FakeAgents::default())
    }

    /// A harness whose core launches agents with `launcher` (a real process launcher, say).
    /// `agents` then records nothing; use `wait_for` to collect events from real processes.
    pub fn launch_with(launcher: Arc<dyn AgentLauncher>) -> Self {
        let data_dir = TempDir::new().unwrap();
        let (tx, events) = channel();
        let core = Core::open_with(data_dir.path(), tx, Runtime { agents: launcher, now_ms: || NOW_MS })
            .expect("core opens");
        Harness { core, events, data_dir, agents: FakeAgents::default() }
    }

    /// Collect events until `done` holds for the collected list (or panic after 20 s).
    pub fn wait_for(&mut self, done: impl Fn(&[Event]) -> bool) -> Vec<Event> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut seen = Vec::new();
        while !done(&seen) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) => seen.push(event),
                Err(_) => panic!("timed out; events so far: {seen:#?}"),
            }
        }
        seen
    }

    fn launch_in(data_dir: TempDir, agents: FakeAgents) -> Self {
        let (tx, events) = channel();
        let runtime = Runtime { agents: Arc::new(agents.clone()), now_ms: || NOW_MS };
        let core = Core::open_with(data_dir.path(), tx, runtime).expect("core opens");
        Harness { core, events, data_dir, agents }
    }

    /// Quit and reopen: a new core on the same data directory.
    pub fn relaunch(self) -> Self {
        let Harness { core, data_dir, .. } = self;
        drop(core);
        Self::launch_in(data_dir, FakeAgents::default())
    }

    /// The core's data directory (where a test may drop a `price-table.json`).
    pub fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    pub fn send(&mut self, command: Command) -> Vec<Event> {
        self.core.dispatch(command);
        self.drain()
    }

    /// Dispatch without collecting events: for real agent processes, collect with `wait_for`.
    pub fn dispatch(&mut self, command: Command) {
        self.core.dispatch(command);
    }

    pub fn drain(&mut self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    pub fn add_repository(&mut self, path: &Path) -> Vec<Event> {
        self.send(Command::AddRepository { path: path.to_string_lossy().into_owned() })
    }

    /// Start a Claude Code session in Ask first.
    pub fn start_session(&mut self, worktree: &Path, prompt: &str) -> Vec<Event> {
        self.start_session_in(worktree, PermissionMode::AskFirst, prompt)
    }

    pub fn start_session_in(&mut self, worktree: &Path, mode: PermissionMode, prompt: &str) -> Vec<Event> {
        self.send(Command::StartSession {
            worktree_path: worktree.to_string_lossy().into_owned(),
            agent: AgentKind::ClaudeCode,
            permission_mode: mode,
            prompt: prompt.into(),
        })
    }

    /// The n-th launched agent (0-based) emits a protocol event.
    pub fn agent_says(&mut self, agent: usize, event: AgentEvent) -> Vec<Event> {
        self.agents.deliver(agent, AgentOutput::Event(event));
        self.drain()
    }

    pub fn agent_exits(&mut self, agent: usize, code: Option<i32>, stderr: &str) -> Vec<Event> {
        self.agents.deliver(agent, AgentOutput::Exited { code, stderr: stderr.into() });
        self.drain()
    }

    /// Deliver every event in `events` from the n-th agent; returns all core events caused.
    pub fn agent_replays(&mut self, agent: usize, events: &[AgentEvent]) -> Vec<Event> {
        events.iter().flat_map(|e| self.agent_says(agent, e.clone())).collect()
    }
}

/// Fake agent transport: records launches and commands; events are delivered by the test.
#[derive(Clone, Default)]
pub struct FakeAgents {
    inner: Arc<Mutex<FakeAgentsInner>>,
}

#[derive(Default)]
struct FakeAgentsInner {
    launched: Vec<(LaunchSpec, AgentSink, Arc<Mutex<Vec<adapter_protocol::Command>>>)>,
    refuse_next: Option<String>,
}

impl FakeAgents {
    pub fn launches(&self) -> Vec<LaunchSpec> {
        self.inner.lock().unwrap().launched.iter().map(|(s, _, _)| s.clone()).collect()
    }

    /// Protocol commands the core sent to the n-th agent.
    pub fn commands(&self, agent: usize) -> Vec<adapter_protocol::Command> {
        self.inner.lock().unwrap().launched[agent].2.lock().unwrap().clone()
    }

    /// The next launch fails with `reason` (the CLI or the sidecar could not start).
    pub fn refuse_next_launch(&self, reason: &str) {
        self.inner.lock().unwrap().refuse_next = Some(reason.into());
    }

    fn deliver(&self, agent: usize, output: AgentOutput) {
        let sink = self.inner.lock().unwrap().launched[agent].1.clone();
        sink(output);
    }
}

struct FakeHandle(Arc<Mutex<Vec<adapter_protocol::Command>>>);

impl AgentHandle for FakeHandle {
    fn send(&mut self, command: adapter_protocol::Command) -> Result<(), String> {
        self.0.lock().unwrap().push(command);
        Ok(())
    }
}

impl AgentLauncher for FakeAgents {
    fn launch(&self, spec: LaunchSpec, sink: AgentSink) -> Result<Box<dyn AgentHandle>, String> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(reason) = inner.refuse_next.take() {
            return Err(reason);
        }
        let commands = Arc::new(Mutex::new(Vec::new()));
        inner.launched.push((spec, sink, commands.clone()));
        Ok(Box::new(FakeHandle(commands)))
    }
}

/// The expected events of a recorded adapter fixture, e.g. `claude-code/first-session`.
pub fn recorded_events(fixture: &str) -> Vec<AgentEvent> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../adapter-protocol/fixtures")
        .join(fixture);
    adapter_protocol::conformance::Fixture::load(dir).expect("fixture loads").expected_events
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

/// The id of the single Session started in `events`.
pub fn started_session(events: &[Event]) -> String {
    let ids: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::SessionStarted { session } => Some(session.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 1, "expected exactly one SessionStarted in {events:#?}");
    ids.into_iter().next().unwrap()
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

/// `git status --porcelain` plus every tracked and untracked file's bytes: a snapshot that
/// changes if anything in the worktree does.
pub fn worktree_snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let out = Process::new("git").args(["status", "--porcelain", "-uall"]).current_dir(dir).output().unwrap();
    let mut snap = vec![("status".to_string(), out.stdout)];
    let files = Process::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(dir)
        .output()
        .unwrap();
    for f in String::from_utf8_lossy(&files.stdout).lines() {
        snap.push((f.to_string(), std::fs::read(dir.join(f)).unwrap_or_default()));
    }
    snap
}
