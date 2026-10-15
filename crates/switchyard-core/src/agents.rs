//! How the core runs agents. A [`AgentLauncher`] starts one agent process for a Session
//! and hands back an [`AgentHandle`] that takes adapter-protocol commands; everything the
//! agent reports comes back through the [`AgentSink`] as protocol events, on whatever
//! thread the launcher reads them on. The core never parses CLI output itself (ADR-0002):
//! launchers speak the adapter protocol (the Claude Code sidecar does so natively).

use std::path::PathBuf;
use std::sync::Arc;

use switchyard_schema::{AgentKind, PermissionMode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub agent: AgentKind,
    /// The worktree the agent runs in.
    pub cwd: PathBuf,
    /// The mode the user asked for; the launcher maps it onto the CLI's own.
    pub permission_mode: PermissionMode,
    /// The resolved Grants, translated into the CLI's own allow rules (Claude Code
    /// `--allowedTools`), so it stops asking for what is already granted.
    pub allowed_tools: Vec<String>,
    /// The resolved denials, as the CLI's deny rules (`--disallowedTools`).
    pub disallowed_tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentOutput {
    Event(adapter_protocol::Event),
    /// The agent wrote a line that is not a protocol event (kept verbatim for the compat log).
    Unreadable { line: String },
    /// The agent process ended. `stderr` is its last output there, for the transcript.
    Exited { code: Option<i32>, stderr: String },
}

/// Where a launched agent delivers its output. May be called from any thread.
pub type AgentSink = Arc<dyn Fn(AgentOutput) + Send + Sync>;

pub trait AgentHandle: Send {
    fn send(&mut self, command: adapter_protocol::Command) -> Result<(), String>;

    /// The agent's process id, recorded (with its start time) so that a relaunch after a
    /// crash can kill the orphan. `None` when there is no process of its own.
    fn pid(&self) -> Option<u32> {
        None
    }
}

pub trait AgentLauncher: Send + Sync {
    /// Start an agent. An `Err` is shown to the user as the reason the session did not start.
    fn launch(&self, spec: LaunchSpec, sink: AgentSink) -> Result<Box<dyn AgentHandle>, String>;
}

/// The launcher used when none is configured: every launch is refused.
pub struct NoAgents;

impl AgentLauncher for NoAgents {
    fn launch(&self, _: LaunchSpec, _: AgentSink) -> Result<Box<dyn AgentHandle>, String> {
        Err("no agent runtime is configured".into())
    }
}
