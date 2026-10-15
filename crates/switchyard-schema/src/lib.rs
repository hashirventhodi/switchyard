//! The one typed stream between the Switchyard core and the renderer.
//!
//! Every [`Command`] the renderer may send and every [`Event`] the core may emit lives
//! here. TypeScript is generated from these types into `ui/src/schema/` by ts-rs; run
//! `cargo test -p switchyard-schema` to regenerate it, and commit the result.
//!
//! Wire format: enums are internally tagged with `type` (`{"type":"addRepository",...}`),
//! fields are camelCase.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A request from the renderer to the core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Command {
    /// The renderer has (re)connected. The core answers with [`Event::Attached`] and then
    /// replays every known Repository, re-reading its worktrees from git, and every Session.
    Attach,
    /// Start tracking the git repository containing `path`.
    AddRepository { path: String },
    /// Start a Session of `agent` on an existing worktree with an initial prompt.
    StartSession {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        agent: AgentKind,
        #[serde(rename = "permissionMode")]
        #[ts(rename = "permissionMode")]
        permission_mode: PermissionMode,
        prompt: String,
    },
    /// Stop (interrupt) a Session: it becomes Idle ("Paused"); the worktree is untouched.
    StopSession {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
    },
    /// Send the next prompt to a Session that is waiting for one.
    SendPrompt {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        text: String,
    },
    /// Open a shell terminal (a PTY running the user's shell) in a worktree, sized in cells.
    OpenShell {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        cols: u16,
        rows: u16,
    },
    /// Keystrokes (or pasted text) typed into a shell terminal.
    WriteShell {
        #[serde(rename = "shellId")]
        #[ts(rename = "shellId")]
        shell_id: String,
        data: String,
    },
    /// The shell terminal's visible size changed.
    ResizeShell {
        #[serde(rename = "shellId")]
        #[ts(rename = "shellId")]
        shell_id: String,
        cols: u16,
        rows: u16,
    },
    /// Close one shell terminal, killing its shell.
    CloseShell {
        #[serde(rename = "shellId")]
        #[ts(rename = "shellId")]
        shell_id: String,
    },
    /// Close every shell terminal on a worktree (the worktree is going away).
    CloseWorktreeShells {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
    },
}

/// Something the core reports. The renderer derives all of its state from these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Event {
    /// Start of a full replay: the renderer drops what it holds and rebuilds from the
    /// events that follow.
    Attached,
    /// A Repository is known to the core (newly added, or replayed on attach).
    RepositoryAdded { repository: Repository },
    /// A worktree of a known Repository, as `git worktree list` reports it.
    WorktreeAdded {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        worktree: Worktree,
    },
    /// The core would not carry out a command. `reason` is shown to the user as is.
    CommandRefused { command: String, reason: String },
    /// A Session exists (newly started, or replayed on attach with its current status).
    SessionStarted { session: Session },
    SessionStatusChanged {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        status: SessionStatus,
    },
    /// What the agent reported about itself, plus running counters. Replaces earlier facts.
    SessionFactsChanged {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        facts: SessionFacts,
    },
    /// Transcript entry number `index` of a Session. An index equal to the transcript's
    /// length appends; a lower one replaces that entry (a tool call gaining its result).
    TranscriptEntry {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        index: u32,
        entry: Entry,
    },
    /// Adapter-protocol event number `index` of a Session, exactly as the adapter sent it
    /// (JSON). For the Agent log's debug view; replayed on attach.
    AgentEventReceived {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        index: u32,
        json: String,
    },
    /// A shell terminal exists (newly opened, or replayed on attach).
    ShellOpened { shell: Shell },
    /// Output from a shell terminal, as text (escape sequences included). On attach, a
    /// shell's recent output is replayed as one of these after its [`Event::ShellOpened`].
    ShellOutput {
        #[serde(rename = "shellId")]
        #[ts(rename = "shellId")]
        shell_id: String,
        data: String,
    },
    /// A shell terminal is gone: closed by the user, its worktree closed, or its shell exited
    /// (`exit_code` is then the shell's, when known).
    ShellClosed {
        #[serde(rename = "shellId")]
        #[ts(rename = "shellId")]
        shell_id: String,
        #[serde(rename = "exitCode")]
        #[ts(rename = "exitCode")]
        exit_code: Option<u32>,
    },
}

/// A shell terminal in the dock: a PTY running the user's shell in a worktree. Shells
/// belong to the worktree, never to a Session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Shell {
    pub id: String,
    pub worktree_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Repository {
    pub id: String,
    /// Display name: the primary worktree's directory name.
    pub name: String,
    /// Absolute path of the primary worktree.
    pub path: String,
    pub base_branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Worktree {
    /// Absolute path of the worktree.
    pub path: String,
    /// The checked-out branch, or `None` when HEAD is detached.
    pub branch: Option<String>,
    /// True for the Repository's primary worktree (its main checkout).
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AgentKind {
    ClaudeCode,
}

/// How much a Session asks before acting. Explore is read-only and the only mode allowed
/// on a Primary worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PermissionMode {
    Explore,
    AskFirst,
    AutoEdit,
    FullAuto,
}

/// Exactly five statuses, shown everywhere with the same colour and label:
/// Running, Waiting ("Needs you"), Idle ("Paused"), Done ("Finished"), Failed ("Error").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SessionStatus {
    Running,
    Waiting,
    Idle,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Session {
    pub id: String,
    pub repository_id: String,
    pub worktree_path: String,
    pub branch: Option<String>,
    pub agent: AgentKind,
    /// The mode the user asked for (the CLI's actual mode is in [`SessionFacts`]).
    pub permission_mode: PermissionMode,
    pub status: SessionStatus,
    /// Milliseconds since the Unix epoch.
    #[ts(type = "number")]
    pub started_at_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionFacts {
    pub model: Option<String>,
    pub cli_version: Option<String>,
    /// The CLI's own name for the mode it actually started in.
    pub cli_permission_mode: Option<String>,
    /// Completed turns.
    pub turns: u32,
    /// Cost reported by the CLI so far, when it reports one.
    pub cost_usd: Option<f64>,
}

/// One transcript entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Entry {
    /// A prompt the user sent.
    User { text: String },
    /// Agent prose.
    Agent { text: String },
    /// A tool call; `state` and `output` change when it finishes.
    Tool {
        #[serde(rename = "callId")]
        #[ts(rename = "callId")]
        call_id: String,
        tool: String,
        /// One line: the command for a shell run, the path for a file tool, and so on.
        summary: String,
        state: ToolState,
        output: Option<String>,
    },
    /// A note from Switchyard itself.
    System { text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ToolState {
    Running,
    Ok,
    Failed,
}
