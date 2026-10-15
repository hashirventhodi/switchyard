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
//!
//! Agent detection is fake too: `harness.probe` answers for the installed CLI's
//! `--version` and the adapter's manifest (Claude Code 2.1.300, protocol `^0.1` unless a
//! test changes them). `compat_log()` reads the compat log the core wrote on disk.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use adapter_protocol::Event as AgentEvent;
use adapter_protocol::AdapterManifest;
use switchyard_core::agents::{AgentHandle, AgentLauncher, AgentOutput, AgentSink, LaunchSpec};
use switchyard_core::compat::AgentProbe;
use switchyard_core::processes::ProcessTable;
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
    pub probe: FakeProbe,
    /// The machine's process table: survives `relaunch`, as real processes outlive the app.
    pub processes: FakeProcesses,
    /// The OS keychain and Switchyard's environment, as secrets see them: survives `relaunch`.
    pub secrets: FakeSecrets,
}

impl Harness {
    pub fn launch() -> Self {
        Self::launch_in(TempDir::new().unwrap(), FakeProcesses::default(), FakeSecrets::default())
    }

    /// A harness whose core launches agents with `launcher` (a real process launcher, say).
    /// `agents` then records nothing; use `wait_for` to collect events from real processes.
    pub fn launch_with(launcher: Arc<dyn AgentLauncher>) -> Self {
        let data_dir = TempDir::new().unwrap();
        let (tx, events) = channel();
        let probe = FakeProbe::default();
        let processes = FakeProcesses::default();
        let secrets = FakeSecrets::default();
        let runtime = Runtime {
            agents: launcher,
            now_ms: || NOW_MS,
            probe: Arc::new(probe.clone()),
            worktree_root: worktree_root_in(&data_dir),
            processes: Arc::new(processes.clone()),
            export_dir: export_dir_in(&data_dir),
            secrets: Arc::new(secrets.clone()),
        };
        let core = Core::open_with(data_dir.path(), tx, runtime).expect("core opens");
        Harness { core, events, data_dir, agents: FakeAgents::default(), probe, processes, secrets }
    }

    /// The Switchyard worktree root this core creates worktrees under (canonical path).
    pub fn worktree_root(&self) -> PathBuf {
        worktree_root_in(&self.data_dir)
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

    fn launch_in(data_dir: TempDir, processes: FakeProcesses, secrets: FakeSecrets) -> Self {
        Self::launch_at(data_dir, processes, secrets, || NOW_MS)
    }

    fn launch_at(data_dir: TempDir, processes: FakeProcesses, secrets: FakeSecrets, now_ms: fn() -> u64) -> Self {
        let (tx, events) = channel();
        let probe = FakeProbe::default();
        let agents = FakeAgents::with_processes(processes.clone());
        let runtime = Runtime {
            agents: Arc::new(agents.clone()),
            now_ms,
            probe: Arc::new(probe.clone()),
            worktree_root: worktree_root_in(&data_dir),
            processes: Arc::new(processes.clone()),
            export_dir: export_dir_in(&data_dir),
            secrets: Arc::new(secrets.clone()),
        };
        let core = Core::open_with(data_dir.path(), tx, runtime).expect("core opens");
        Harness { core, events, data_dir, agents, probe, processes, secrets }
    }

    /// Quit and reopen: a new core on the same data directory. Agent processes the old core
    /// started are still in the process table (the app crashed or was killed).
    pub fn relaunch(self) -> Self {
        let Harness { core, data_dir, processes, secrets, .. } = self;
        drop(core);
        Self::launch_in(data_dir, processes, secrets)
    }

    /// Relaunch with the wall clock at `now_ms` from then on.
    pub fn relaunch_at(self, now_ms: fn() -> u64) -> Self {
        let Harness { core, data_dir, processes, secrets, .. } = self;
        drop(core);
        Self::launch_at(data_dir, processes, secrets, now_ms)
    }

    /// Where `exportTranscript` writes files.
    pub fn export_dir(&self) -> PathBuf {
        export_dir_in(&self.data_dir)
    }

    /// The core's data directory (where a test may drop a `price-table.json`).
    pub fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    pub fn send(&mut self, command: Command) -> Vec<Event> {
        self.core.dispatch(command);
        self.drain()
    }

    /// Shell terminals run `program` with no arguments (e.g. `/bin/sh`: no rc files).
    pub fn use_shell(&mut self, program: &str) {
        self.core.set_shell(PathBuf::from(program), Vec::new());
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

    /// The n-th agent writes a line that is not a protocol event.
    pub fn agent_writes(&mut self, agent: usize, line: &str) -> Vec<Event> {
        self.agents.deliver(agent, AgentOutput::Unreadable { line: line.into() });
        self.drain()
    }

    /// Every record in the compat log, oldest first.
    pub fn compat_log(&self) -> Vec<serde_json::Value> {
        let text = std::fs::read_to_string(self.data_dir.path().join("compat-log.jsonl")).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str(l).expect("compat log lines are JSON")).collect()
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

/// The worktree root inside a harness data directory: outside every `TempRepo`.
fn worktree_root_in(data_dir: &TempDir) -> PathBuf {
    let root = data_dir.path().join("worktrees");
    std::fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

fn export_dir_in(data_dir: &TempDir) -> PathBuf {
    data_dir.path().join("exports")
}

/// Fake agent transport: records launches and commands; events are delivered by the test.
/// Every launch starts a fake process (pid 4001, 4002, ...) in the shared process table.
#[derive(Clone, Default)]
pub struct FakeAgents {
    inner: Arc<Mutex<FakeAgentsInner>>,
    processes: FakeProcesses,
}

#[derive(Default)]
struct FakeAgentsInner {
    launched: Vec<(LaunchSpec, AgentSink, Arc<Mutex<Vec<adapter_protocol::Command>>>)>,
    refuse_next: Option<String>,
}

impl FakeAgents {
    fn with_processes(processes: FakeProcesses) -> Self {
        FakeAgents { inner: Arc::default(), processes }
    }

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

struct FakeHandle(Arc<Mutex<Vec<adapter_protocol::Command>>>, u32);

impl AgentHandle for FakeHandle {
    fn send(&mut self, command: adapter_protocol::Command) -> Result<(), String> {
        self.0.lock().unwrap().push(command);
        Ok(())
    }

    fn pid(&self) -> Option<u32> {
        Some(self.1)
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
        let pid = self.processes.spawn();
        Ok(Box::new(FakeHandle(commands, pid)))
    }
}

/// A fake process table: pid -> start time. Agent launches add processes; a test may end
/// them or let another program reuse a pid; the core asks it for start times and kills.
#[derive(Clone, Default)]
pub struct FakeProcesses {
    inner: Arc<Mutex<FakeProcessesInner>>,
}

#[derive(Default)]
struct FakeProcessesInner {
    running: std::collections::BTreeMap<u32, String>,
    /// Pids that ended (or were killed); any other pid this table never heard of is a real
    /// process the core started (a shell PTY), reported as running since `started-<pid>`.
    ended: std::collections::BTreeSet<u32>,
    spawned: u32,
    killed: Vec<u32>,
}

impl FakeProcesses {
    /// Start a process; returns its pid (4001, 4002, ...).
    pub fn spawn(&self) -> u32 {
        let mut inner = self.inner.lock().unwrap();
        inner.spawned += 1;
        let pid = 4000 + inner.spawned;
        inner.running.insert(pid, format!("started-{pid}"));
        pid
    }

    /// The process `pid` ended and another program now runs under the same pid.
    pub fn reuse_pid(&self, pid: u32) {
        self.inner.lock().unwrap().running.insert(pid, format!("reused-{pid}"));
    }

    /// The process `pid` ended.
    pub fn end(&self, pid: u32) {
        let mut inner = self.inner.lock().unwrap();
        inner.running.remove(&pid);
        inner.ended.insert(pid);
    }

    pub fn is_running(&self, pid: u32) -> bool {
        self.start_time(pid).is_some()
    }

    /// Every pid the core killed, in order.
    pub fn killed(&self) -> Vec<u32> {
        self.inner.lock().unwrap().killed.clone()
    }
}

impl ProcessTable for FakeProcesses {
    fn start_time(&self, pid: u32) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        if inner.ended.contains(&pid) {
            return None;
        }
        Some(inner.running.get(&pid).cloned().unwrap_or_else(|| format!("started-{pid}")))
    }

    fn kill(&self, pid: u32) {
        let mut inner = self.inner.lock().unwrap();
        inner.running.remove(&pid);
        inner.ended.insert(pid);
        inner.killed.push(pid);
    }
}

/// Fake agent detection. Starts as an installed, supported Claude Code with a matching adapter.
#[derive(Clone)]
pub struct FakeProbe {
    inner: Arc<Mutex<FakeProbeInner>>,
}

struct FakeProbeInner {
    cli: Result<Option<String>, String>,
    manifest: Result<AdapterManifest, String>,
    probes: usize,
}

impl Default for FakeProbe {
    fn default() -> Self {
        let manifest = AdapterManifest { name: "claude-code".into(), implements: "^0.1".parse().unwrap() };
        FakeProbe {
            inner: Arc::new(Mutex::new(FakeProbeInner {
                cli: Ok(Some("2.1.300 (Claude Code)".into())),
                manifest: Ok(manifest),
                probes: 0,
            })),
        }
    }
}

impl FakeProbe {
    /// What `claude --version` prints from now on; `None`: not installed.
    pub fn cli_prints(&self, output: Option<&str>) {
        self.inner.lock().unwrap().cli = Ok(output.map(str::to_owned));
    }

    /// The adapter now declares it implements `range`.
    pub fn adapter_implements(&self, range: &str) {
        let manifest = AdapterManifest { name: "claude-code".into(), implements: range.parse().unwrap() };
        self.inner.lock().unwrap().manifest = Ok(manifest);
    }

    /// Asking the adapter for its manifest fails (its runtime cannot start).
    pub fn adapter_fails(&self, reason: &str) {
        self.inner.lock().unwrap().manifest = Err(reason.into());
    }

    /// How many times the CLI version was probed.
    pub fn probes(&self) -> usize {
        self.inner.lock().unwrap().probes
    }
}

impl AgentProbe for FakeProbe {
    fn cli_version(&self, _: AgentKind) -> Result<Option<String>, String> {
        let mut inner = self.inner.lock().unwrap();
        inner.probes += 1;
        inner.cli.clone()
    }

    fn adapter_manifest(&self, _: AgentKind) -> Result<AdapterManifest, String> {
        self.inner.lock().unwrap().manifest.clone()
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

/// A fake OS keychain plus a fake Switchyard environment: tests never touch the real
/// keychain or depend on the real environment.
#[derive(Clone, Default)]
pub struct FakeSecrets {
    inner: Arc<Mutex<FakeSecretsInner>>,
}

#[derive(Default)]
struct FakeSecretsInner {
    keychain: std::collections::BTreeMap<String, String>,
    environment: std::collections::BTreeMap<String, String>,
}

impl FakeSecrets {
    /// The keychain holds `value` for `name`.
    pub fn in_keychain(&self, name: &str, value: &str) {
        self.inner.lock().unwrap().keychain.insert(name.into(), value.into());
    }

    /// Switchyard was started with `name=value` in its environment.
    pub fn in_environment(&self, name: &str, value: &str) {
        self.inner.lock().unwrap().environment.insert(name.into(), value.into());
    }

    /// What the keychain holds for `name`.
    pub fn keychain(&self, name: &str) -> Option<String> {
        self.inner.lock().unwrap().keychain.get(name).cloned()
    }
}

impl switchyard_core::secrets::SecretValues for FakeSecrets {
    fn keychain_get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.keychain(name))
    }

    fn keychain_set(&self, name: &str, value: &str) -> Result<(), String> {
        self.in_keychain(name, value);
        Ok(())
    }

    fn environment(&self, name: &str) -> Option<String> {
        self.inner.lock().unwrap().environment.get(name).cloned()
    }
}
