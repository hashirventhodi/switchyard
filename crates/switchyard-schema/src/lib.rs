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
    /// Create a worktree of a Repository on a new `branch` from its base branch, in the
    /// Switchyard worktree root (or the root the repository's policy names).
    CreateWorktree {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        branch: String,
    },
    /// "Create new from base": create a worktree as [`Command::CreateWorktree`] does, then
    /// start a Session on it as [`Command::StartSession`] does.
    StartSessionOnNewWorktree {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        branch: String,
        agent: AgentKind,
        #[serde(rename = "permissionMode")]
        #[ts(rename = "permissionMode")]
        permission_mode: PermissionMode,
        prompt: String,
    },
    /// Remove a clean, unoccupied, non-primary worktree (`git worktree remove`). The
    /// branch is kept.
    RemoveWorktree {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
    },
    /// Re-read every worktree's facts (ahead/behind, dirty count) from git.
    RefreshWorktrees,
    /// Detect the installed agent CLIs again (after installing or updating one). The core
    /// answers with [`Event::AgentsChecked`].
    CheckAgents,
    /// Read the worktree's uncommitted changes (working tree against the index, untracked
    /// files as added) with the review decisions that still apply. The core answers with
    /// [`Event::ChangesRead`] and the worktree's facts.
    ReadChanges {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
    },
    /// Accept or Reject a changed file as a whole (`null` clears the decision). The decision
    /// holds only while the file's diff stays as it was when decided.
    DecideFile {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        path: String,
        decision: Option<Decision>,
    },
    /// Stage and commit every accepted file whole, with `message`. Undecided and rejected
    /// files stay in the worktree untouched; every decision on the worktree is discarded.
    CommitAccepted {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        message: String,
    },
    /// Answer a Session's pending Permission request. Every surface (toast, inbox, request
    /// card) sends this; the first answer wins and any later one is refused.
    AnswerRequest {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "requestId")]
        #[ts(rename = "requestId")]
        request_id: String,
        verdict: Verdict,
    },
}

/// The user's answer to a Permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Verdict {
    /// Run once: approve this request without creating a Grant.
    AllowOnce,
    /// Skip: the agent carries on without it.
    Deny,
    /// The answer to a question: one of its options, or the user's own words.
    Answer { text: String },
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
    /// What a Session has spent so far, as of `atMs`. Each one supersedes the Session's
    /// previous one (the totals are running totals); the renderer keeps them all so it can
    /// tell what was spent since a given time (Spend today). Replayed in full on attach,
    /// with estimates recomputed from the current price table.
    SpendRecorded {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        /// Milliseconds since the Unix epoch.
        #[serde(rename = "atMs")]
        #[ts(rename = "atMs", type = "number")]
        at_ms: u64,
        spend: Spend,
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
    /// Whether each agent this build knows can start a Session: its CLI is installed, its
    /// version is inside the range this build was recorded against, and its adapter speaks
    /// this build's protocol. Sent on attach and after [`Command::CheckAgents`]; replaces
    /// earlier results.
    AgentsChecked { agents: Vec<AgentSetup> },
    /// A worktree is gone (removed through Switchyard).
    WorktreeRemoved {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
    },
    /// What git says about a worktree right now. Replaces earlier facts for that path.
    WorktreeFactsChanged {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        facts: WorktreeFacts,
    },
    /// A worktree's uncommitted changes as git reports them now. Replaces earlier ones.
    ChangesRead {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        files: Vec<ChangedFile>,
    },
    /// The decision on one changed file, after [`Command::DecideFile`].
    FileDecided {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        path: String,
        decision: Option<Decision>,
    },
    /// Accepted files were committed on the worktree's branch.
    Committed {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        branch: Option<String>,
        /// Abbreviated commit id.
        commit: String,
        /// Paths committed (for a rename, the new path).
        files: Vec<String>,
    },
}

/// A review decision on a changed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Decision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    /// Unmerged in the index: git stopped on a conflict here.
    Conflicted,
}

/// One file with uncommitted changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChangedFile {
    pub path: String,
    /// The path in the index, for a rename.
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// Lines added and removed.
    pub added: u32,
    pub removed: u32,
    /// Git shows no text diff for the file.
    pub binary: bool,
    pub decision: Option<Decision>,
    pub hunks: Vec<Hunk>,
}

/// One hunk of a file's diff. `id` is stable while the hunk's own lines are unchanged:
/// it hashes the index-side path and range with the hunk's lines, so edits elsewhere in
/// the file and renaming the file leave it alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Hunk {
    pub id: String,
    /// The `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number on the index side (context and removed lines).
    pub old: Option<u32>,
    /// Line number on the working-tree side (context and added lines).
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// A worktree's state relative to its Repository's base branch, read from git.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorktreeFacts {
    /// Commits on the worktree's HEAD that the base branch lacks.
    pub ahead: u32,
    /// Commits on the base branch that the worktree's HEAD lacks.
    pub behind: u32,
    /// Uncommitted files: modified, staged, deleted or untracked.
    pub dirty: u32,
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

/// One agent's readiness, as detected on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentSetup {
    pub agent: AgentKind,
    /// Display name, e.g. "Claude Code".
    pub name: String,
    pub state: AgentSetupState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum AgentSetupState {
    /// Installed, in range: New session may start it.
    Ready { version: String },
    /// The CLI is not installed. Shown as a setup state on the Dashboard.
    Missing {
        /// What to run to install it.
        #[serde(rename = "installHint")]
        #[ts(rename = "installHint")]
        install_hint: String,
        link: Link,
    },
    /// Installed but not usable with this build (version outside the recorded range, or an
    /// adapter that cannot start or speaks another protocol). New session is blocked for
    /// this agent; `message` says why and `link` where to get an update.
    Unsupported { version: Option<String>, message: String, link: Option<Link> },
}

/// A web page the user may open in their browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Link {
    pub label: String,
    pub url: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
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
}

/// A Session's running spend. Cost is either reported by the CLI or, for a CLI that
/// reports tokens only, estimated from the price table when read (shown with a tilde);
/// the two are never mixed into one figure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Spend {
    /// Input tokens, not counting cache reads.
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    #[ts(type = "number")]
    pub cache_read_tokens: u64,
    /// Cost the CLI reported, in USD.
    pub reported_usd: Option<f64>,
    /// Cost estimated from the price table, in USD: only when the CLI reported none and the
    /// table prices the Session's model.
    pub estimated_usd: Option<f64>,
    /// The price table version `estimatedUsd` was computed with.
    pub price_table_version: Option<String>,
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
    /// A Permission request. Replaced in place when it is answered or cancelled.
    Request {
        #[serde(rename = "requestId")]
        #[ts(rename = "requestId")]
        request_id: String,
        request: Request,
        /// Why the agent asks, when it says.
        reason: Option<String>,
        state: RequestState,
        /// What the user answered, for an answered question.
        answer: Option<String>,
    },
}

/// What a Permission request asks for: exactly five kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Request {
    Command { command: String, cwd: Option<String> },
    FileWrite { path: String, diff: Option<String> },
    Network { domain: String, url: Option<String> },
    /// `input` is the tool's arguments as JSON text.
    McpTool { server: String, tool: String, input: String },
    Question { question: String, options: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RequestState {
    /// Waiting for the user.
    Pending,
    /// Run once.
    Allowed,
    /// Skipped.
    Denied,
    /// A question the user answered.
    Answered,
    /// Withdrawn unanswered: the session was stopped.
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ToolState {
    Running,
    Ok,
    Failed,
}
