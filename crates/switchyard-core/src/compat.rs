//! Compatibility: which agent CLIs are installed, whether their versions are inside the
//! ranges this build was recorded against (`compat.json`, the release manifest's ranges),
//! and the compat log.
//!
//! Detection runs on the first attach (or the first session start, whichever comes first)
//! and again on [`Command::CheckAgents`](switchyard_schema::Command::CheckAgents); New
//! session is refused for an agent that is not ready. Nothing here touches the network:
//! the only processes run are the CLI's `--version` and the adapter's `--manifest`.
//!
//! The compat log is `compat-log.jsonl` in the data directory: one JSON object per adapter
//! failure, only ever appended, never uploaded. Each record carries the CLI's name and
//! version, the protocol version this build speaks, the range the adapter declares, and a
//! failure class (`spawn_failure`, `protocol_mismatch`, `parse_error`).

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use adapter_protocol::{AdapterManifest, Event as AgentEvent, PROTOCOL_VERSION};
use semver::Version;
use serde::Deserialize;
use serde_json::json;
use switchyard_schema::{AgentKind, AgentSetup, AgentSetupState, Event, Link};

use crate::Inner;

/// How the core finds out what is installed. The app uses [`SystemProbe`]; tests fake it.
pub trait AgentProbe: Send + Sync {
    /// What the agent's CLI prints for `--version`, or `Ok(None)` when it is not installed.
    fn cli_version(&self, agent: AgentKind) -> Result<Option<String>, String>;
    /// The manifest the agent's adapter declares. An `Err` means the adapter could not run.
    fn adapter_manifest(&self, agent: AgentKind) -> Result<AdapterManifest, String>;
}

/// Detection when none is configured: nothing is installed.
pub struct NoProbe;

impl AgentProbe for NoProbe {
    fn cli_version(&self, _: AgentKind) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn adapter_manifest(&self, _: AgentKind) -> Result<AdapterManifest, String> {
        Err("no adapter runtime is configured".into())
    }
}

/// Detection on this machine: looks for the CLI in `search_dirs` and runs its `--version`;
/// asks the sidecar for its manifest with `<node> <sidecar> --manifest`. The Codex adapter
/// runs in-process, so its manifest is this build's.
pub struct SystemProbe {
    pub search_dirs: Vec<PathBuf>,
    pub node: PathBuf,
    pub sidecar: PathBuf,
}

/// Longest a `--version` or `--manifest` run may take before it counts as failed.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

impl SystemProbe {
    /// PATH plus the usual install locations (apps launched from Finder get a short PATH).
    pub fn default_search_dirs() -> Vec<PathBuf> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        if let Some(home) = home {
            dirs.push(home.join(".local/bin"));
            dirs.push(home.join(".claude/local"));
        }
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
        dirs
    }
}

impl AgentProbe for SystemProbe {
    fn cli_version(&self, agent: AgentKind) -> Result<Option<String>, String> {
        let executable = &entry(agent).executable;
        let Some(cli) = self.search_dirs.iter().map(|d| d.join(executable)).find(|p| p.is_file()) else {
            return Ok(None);
        };
        run(&cli, &["--version"]).map(Some)
    }

    fn adapter_manifest(&self, agent: AgentKind) -> Result<AdapterManifest, String> {
        if agent == AgentKind::Codex {
            return Ok(codex_adapter::manifest());
        }
        let sidecar = self.sidecar.to_string_lossy();
        let out = run(&self.node, &[&sidecar, "--manifest"])?;
        serde_json::from_str(out.trim()).map_err(|e| format!("the adapter's manifest is unreadable ({e}): {out}"))
    }
}

/// Run `program args`, returning its stdout; an error if it cannot start, fails or hangs.
fn run(program: &Path, args: &[&str]) -> Result<String, String> {
    let shown = format!("{} {}", program.display(), args.join(" "));
    let mut child = Process::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                return Err(format!("{shown} did not answer"));
            }
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    child.stdout.take().map(|mut s| s.read_to_string(&mut stdout));
    child.stderr.take().map(|mut s| s.read_to_string(&mut stderr));
    if !status.success() {
        return Err(format!("{shown} failed: {}", stderr.trim()));
    }
    Ok(stdout)
}

// ---- The compat table (compat.json) ----

#[derive(Deserialize)]
struct Table {
    switchyard_releases: String,
    agents: Vec<AgentEntry>,
}

#[derive(Deserialize)]
struct AgentEntry {
    agent: AgentKind,
    name: String,
    cli_name: String,
    executable: String,
    supported: Range,
    install_hint: String,
    install_url: String,
    update_hint: String,
}

/// `min` inclusive, `below` exclusive.
#[derive(Deserialize)]
struct Range {
    min: Version,
    below: Version,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| serde_json::from_str(include_str!("../compat.json")).expect("compat.json is valid"))
}

fn entry(agent: AgentKind) -> &'static AgentEntry {
    table().agents.iter().find(|a| a.agent == agent).expect("every agent kind is in compat.json")
}

// ---- Detection results and the log ----

pub(crate) struct Compat {
    log: PathBuf,
    checked: Option<Vec<Checked>>,
}

struct Checked {
    setup: AgentSetup,
    cli_version: Option<String>,
    adapter: Option<AdapterManifest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureClass {
    /// The adapter could not be started, or its process ended before the CLI initialised.
    SpawnFailure,
    /// The adapter speaks a protocol version (or event type) this build does not.
    ProtocolMismatch,
    /// The adapter wrote something that is not a well-formed protocol event.
    ParseError,
}

impl FailureClass {
    fn as_str(self) -> &'static str {
        match self {
            FailureClass::SpawnFailure => "spawn_failure",
            FailureClass::ProtocolMismatch => "protocol_mismatch",
            FailureClass::ParseError => "parse_error",
        }
    }
}

impl Compat {
    pub(crate) fn open(data_dir: &Path) -> Compat {
        Compat { log: data_dir.join("compat-log.jsonl"), checked: None }
    }
}

/// On attach: the last detection result, detecting first if nothing has been yet.
pub(crate) fn replay(inner: &mut Inner) {
    if inner.compat.checked.is_none() {
        detect(inner);
    }
    emit_checked(inner);
}

/// Detect again ([`Command::CheckAgents`](switchyard_schema::Command::CheckAgents)).
pub(crate) fn check(inner: &mut Inner) {
    detect(inner);
    emit_checked(inner);
}

/// May a session of `agent` start? `Err` is the reason shown to the user.
pub(crate) fn admit(inner: &mut Inner, agent: AgentKind) -> Result<(), String> {
    if inner.compat.checked.is_none() {
        detect(inner);
    }
    let checked = checked(inner, agent).expect("every agent is detected");
    match &checked.setup.state {
        AgentSetupState::Ready { .. } => Ok(()),
        AgentSetupState::Missing { install_hint, .. } => {
            Err(format!("{} is not installed. Install it with: {install_hint}", checked.setup.name))
        }
        AgentSetupState::Unsupported { message, .. } => Err(message.clone()),
    }
}

/// Append one record to the compat log. `cli_version` is what the agent reported, when it
/// did; otherwise the detected version is used.
pub(crate) fn record(inner: &Inner, agent: AgentKind, class: FailureClass, cli_version: Option<&str>, detail: &str) {
    let found = checked(inner, agent);
    let detected = found.and_then(|c| c.cli_version.as_deref());
    let adapter = found.and_then(|c| c.adapter.as_ref());
    let line = json!({
        "at_ms": (inner.runtime.now_ms)(),
        "agent": adapter.map_or(entry(agent).cli_name.as_str(), |m| m.name.as_str()),
        "cli_name": entry(agent).cli_name,
        "cli_version": cli_version.or(detected),
        "protocol_version": PROTOCOL_VERSION.to_string(),
        "adapter_implements": adapter.map(|m| m.implements.to_string()),
        "failure_class": class.as_str(),
        "detail": detail,
    });
    let written = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&inner.compat.log)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = written {
        eprintln!("switchyard: cannot write the compat log {}: {e}", inner.compat.log.display());
    }
}

/// What kind of failure an unreadable adapter line is: an event type this build does not
/// know means the adapter speaks another protocol; anything else is a parse error.
pub(crate) fn classify_unreadable(line: &str) -> FailureClass {
    match serde_json::from_str::<AgentEvent>(line) {
        Err(e) if e.to_string().starts_with("unknown variant") => FailureClass::ProtocolMismatch,
        _ => FailureClass::ParseError,
    }
}

fn checked(inner: &Inner, agent: AgentKind) -> Option<&Checked> {
    inner.compat.checked.as_ref()?.iter().find(|c| c.setup.agent == agent)
}

fn emit_checked(inner: &Inner) {
    let agents = inner.compat.checked.iter().flatten().map(|c| c.setup.clone()).collect();
    inner.emit(Event::AgentsChecked { agents });
}

fn detect(inner: &mut Inner) {
    let mut results = Vec::new();
    let mut failures = Vec::new();
    for agent in &table().agents {
        let (checked, failure) = detect_one(inner, agent);
        if let Some((class, detail)) = failure {
            failures.push((agent.agent, class, detail));
        }
        results.push(checked);
    }
    inner.compat.checked = Some(results);
    for (agent, class, detail) in failures {
        record(inner, agent, class, None, &detail);
    }
}

fn detect_one(inner: &Inner, agent: &AgentEntry) -> (Checked, Option<(FailureClass, String)>) {
    let probe = &inner.runtime.probe;
    let done = |state, cli_version, adapter| Checked {
        setup: AgentSetup { agent: agent.agent, name: agent.name.clone(), state },
        cli_version,
        adapter,
    };
    let releases = || Link { label: "Get the latest Switchyard".into(), url: table().switchyard_releases.clone() };
    let unsupported = |version: Option<&Version>, message: String, link: Option<Link>| AgentSetupState::Unsupported {
        version: version.map(Version::to_string),
        message,
        link,
    };
    let name = &agent.name;

    let output = match probe.cli_version(agent.agent) {
        Ok(Some(output)) => output,
        Ok(None) => {
            let link = Link { label: format!("How to install {name}"), url: agent.install_url.clone() };
            let state = AgentSetupState::Missing { install_hint: agent.install_hint.clone(), link };
            return (done(state, None, None), None);
        }
        Err(e) => {
            let message = format!("Switchyard could not run {}: {e}", agent.executable);
            return (done(unsupported(None, message, None), None, None), None);
        }
    };
    let Some(version) = parse_version(&output) else {
        let printed = output.trim();
        let message = format!(
            "Switchyard could not read the {name} version from `{} --version` (it printed: {printed})",
            agent.executable
        );
        return (done(unsupported(None, message, None), None, None), None);
    };
    let Range { min, below } = &agent.supported;
    let state = if version >= *below {
        let message = format!(
            "{name} {version} is not yet supported by this build of Switchyard, which was recorded against \
             {min} up to {below} (not included). Check for a Switchyard update."
        );
        Some(unsupported(Some(&version), message, Some(releases())))
    } else if version < *min {
        let message = format!(
            "{name} {version} is older than this build of Switchyard supports ({min} or later). \
             Update it with `{}`.",
            agent.update_hint
        );
        let link = Link { label: format!("How to update {name}"), url: agent.install_url.clone() };
        Some(unsupported(Some(&version), message, Some(link)))
    } else {
        None
    };
    if let Some(state) = state {
        return (done(state, Some(version.to_string()), None), None);
    }

    match probe.adapter_manifest(agent.agent) {
        Err(e) => {
            let message = format!("The {name} adapter could not start: {e}");
            let failure = (FailureClass::SpawnFailure, format!("adapter manifest: {e}"));
            (done(unsupported(Some(&version), message, None), Some(version.to_string()), None), Some(failure))
        }
        Ok(manifest) if !manifest.implements.matches(&PROTOCOL_VERSION) => {
            let message = format!(
                "This build's {name} adapter implements adapter protocol {}, but Switchyard speaks {PROTOCOL_VERSION}. \
                 Reinstall Switchyard.",
                manifest.implements
            );
            let failure = (
                FailureClass::ProtocolMismatch,
                format!("adapter {} implements {}", manifest.name, manifest.implements),
            );
            let state = unsupported(Some(&version), message, Some(releases()));
            (done(state, Some(version.to_string()), Some(manifest)), Some(failure))
        }
        Ok(manifest) => {
            let state = AgentSetupState::Ready { version: version.to_string() };
            (done(state, Some(version.to_string()), Some(manifest)), None)
        }
    }
}

/// The first semver in a `--version` output: `2.1.300 (Claude Code)`, `codex-cli 0.144.1`.
fn parse_version(output: &str) -> Option<Version> {
    output
        .split_whitespace()
        .map(|word| word.trim_start_matches('v').trim_matches(|c: char| !c.is_ascii_alphanumeric()))
        .find_map(|word| Version::parse(word).ok())
}
