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
    /// Accept or Reject one hunk of a changed file (`null` clears it). Rejecting a hunk
    /// clears an Accept on its file: a file with any rejected hunk is not committed until
    /// the agent revises the hunk or the user accepts the whole file. Answered with
    /// [`Event::HunkReviewed`].
    DecideHunk {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        path: String,
        #[serde(rename = "hunkId")]
        #[ts(rename = "hunkId")]
        hunk_id: String,
        decision: Option<Decision>,
    },
    /// Comment on one hunk for the agent (`null` or blank clears the comment). Answered with
    /// [`Event::HunkReviewed`].
    CommentHunk {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        path: String,
        #[serde(rename = "hunkId")]
        #[ts(rename = "hunkId")]
        hunk_id: String,
        comment: Option<String>,
    },
    /// Send every rejected hunk and comment not yet sent to the agent of the worktree's
    /// Session as one structured message. Switchyard changes no file; rejections keep
    /// blocking their files until the agent revises them. Answered with
    /// [`Event::HunkReviewed`] (now `sent`) per hunk and [`Event::FeedbackSent`].
    SendFeedback {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
    },
    /// Rebase a clean, non-primary worktree's branch on its Repository's base. Refused while
    /// its Session is Running. A rebase that stops on conflicts is aborted, leaving the
    /// branch as it was, and refused with the conflicting files named.
    RebaseWorktree {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
    },
    /// Send the Session's agent one message asking it to rebase its branch on base and
    /// resolve the conflicts itself. The Session goes Running.
    AskAgentToRebase {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
    },
    /// Read the commits a worktree's branch has above its base. The core answers with
    /// [`Event::WorktreeHistory`].
    ReadHistory {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
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
    /// Resume a Paused (or Finished) Session: the agent continues with its context. When
    /// its process is gone (after a relaunch, say) the agent is relaunched through its CLI's
    /// own resume-by-id. Nothing ever resumes on its own.
    ResumeSession {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
    },
    /// "Stop and replace": archive a Paused, Finished or Error Session and start a new one
    /// on its worktree. A Running or Needs you Session is never replaced.
    ReplaceSession {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        agent: AgentKind,
        #[serde(rename = "permissionMode")]
        #[ts(rename = "permissionMode")]
        permission_mode: PermissionMode,
        prompt: String,
    },
    /// Read an archived Session's transcript. Answered with [`Event::ArchivedTranscript`].
    ReadArchivedSession {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
    },
    /// Write a Session's transcript (live or archived) as JSONL, one entry per line, to the
    /// export folder. Answered with [`Event::TranscriptExported`].
    ExportTranscript {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
    },
    /// Change the user settings. Answered with [`Event::SettingsChanged`].
    UpdateSettings { settings: Settings },
    /// Read a Repository's Policy (its committed `.switchyard/policy.yml` and the user's
    /// local Grants for it) again. The core answers with [`Event::PolicyChanged`].
    ReadPolicy {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
    },
    /// Revoke one of the user's local Grants. Grants from the repository's policy file are
    /// changed in the file, through a pull request, never here.
    RevokeGrant {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        #[serde(rename = "grantId")]
        #[ts(rename = "grantId")]
        grant_id: String,
    },
}

/// The user's answer to a Permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Verdict {
    /// Run once: approve this request without creating a Grant.
    AllowOnce,
    /// Always allow: approve this request and keep a local Grant for the Repository, so the
    /// next matching request in any of its Sessions is answered without asking. `pattern`
    /// is the command pattern to grant (a command prefix plus a glob on the rest, e.g.
    /// `pnpm test *`); the core suggests one when it is absent and refuses one that does
    /// not cover the request. Network and MCP-tool grants are the request's domain or
    /// server + tool. File writes and questions are never granted.
    AllowAlways { pattern: Option<String> },
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
    /// A Session was archived (replaced, or its worktree removed), or an archived Session
    /// is replayed on attach. It is no longer a live Session.
    SessionArchived { archived: ArchivedSession },
    /// Retention removed an archived Session for good.
    ArchivedSessionPruned {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
    },
    /// An archived Session's full transcript, as asked for.
    ArchivedTranscript {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        entries: Vec<Entry>,
    },
    /// A transcript was written to `path`.
    TranscriptExported {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        path: String,
    },
    /// The user settings (on attach and after a change). Replaces earlier settings.
    SettingsChanged { settings: Settings },
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
    /// One hunk's review as it stands now (decision, comment, whether they were sent to the
    /// agent). Replaces that hunk's earlier review.
    HunkReviewed {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        path: String,
        #[serde(rename = "hunkId")]
        #[ts(rename = "hunkId")]
        hunk_id: String,
        decision: Option<Decision>,
        comment: Option<String>,
        sent: bool,
    },
    /// Hunk feedback went to the Session's agent as one message.
    FeedbackSent {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        /// How many hunks the message covered.
        hunks: u32,
    },
    /// A worktree's branch was rebased on `base` without conflicts.
    WorktreeRebased {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        base: String,
    },
    /// The commits on a worktree's branch that its base lacks, newest first. Replaces
    /// earlier history for that path.
    WorktreeHistory {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        /// The base branch the commits sit above.
        base: String,
        commits: Vec<Commit>,
    },
    /// A Repository's Policy as it stands: sent after [`Command::ReadPolicy`], and whenever
    /// a local Grant is added or revoked. Replaces earlier policy for that Repository.
    PolicyChanged {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        policy: Policy,
    },
}

/// What may run without asking in a Repository, resolved across layers (organisation,
/// the repository's committed policy file, the user's local Grants). A denial in any
/// layer wins over an allow in any other.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Policy {
    /// The Permission mode New session starts in, when the repository's policy names one.
    pub default_mode: Option<PermissionMode>,
    /// The most autonomous Permission mode a Session may run in, when the policy caps it.
    pub ceiling: Option<PermissionMode>,
    /// Every Grant and denial that applies: repository file first, then local.
    pub grants: Vec<Grant>,
    /// Why the repository's policy file could not be read. While it is broken nothing is
    /// granted (its denials are unknown), so every request asks.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Grant {
    pub id: String,
    pub layer: PolicyLayer,
    pub effect: GrantEffect,
    pub rule: GrantRule,
}

/// Where a Grant comes from. Only local Grants can be revoked in Switchyard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PolicyLayer {
    /// Organisation policy (modelled; not synced before M4).
    Org,
    /// The Repository's committed `.switchyard/policy.yml`.
    Repo,
    /// The user's own Grants for the Repository, made with Always allow.
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GrantEffect {
    Allow,
    Deny,
}

/// What a Grant matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum GrantRule {
    /// A command prefix plus a glob on the rest (`git push *`). No shell parsing; a
    /// wildcard never matches an interpreter prefix (`bash -c`, `sh -c`, `eval`, `xargs`,
    /// `env`, `npx`) or a chained command.
    Command { pattern: String },
    /// A network domain (`registry.npmjs.org`; `*.example.com` for its subdomains).
    Network { domain: String },
    /// An MCP server's tool (`*` for every tool of the server).
    McpTool { server: String, tool: String },
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
    /// The hunk's own decision (a file decision overrides it).
    pub decision: Option<Decision>,
    /// The user's comment for the agent.
    pub comment: Option<String>,
    /// The rejection and comment as they stand were already sent to the agent.
    pub sent: bool,
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

/// One commit on a worktree's branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Commit {
    pub hash: String,
    /// Abbreviated hash, as git abbreviates it.
    pub short: String,
    /// The first line of the message.
    pub subject: String,
    pub author: String,
    /// Commit time, milliseconds since the Unix epoch.
    #[ts(type = "number")]
    pub at_ms: u64,
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
    /// Files a merge of the base branch into this branch would conflict on, from a dry-run
    /// merge that never touches the working tree or the index. Empty when not behind.
    pub conflicts: Vec<String>,
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
    /// Always allow: approved and a local Grant kept; `answer` holds what was granted.
    AlwaysAllowed,
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

/// A Session kept as the audit trail after it was replaced or its worktree removed. Belongs
/// to its Repository (keyed by branch and session id); readable, never resumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArchivedSession {
    /// The Session as it was when archived.
    pub session: Session,
    /// Milliseconds since the Unix epoch.
    #[ts(type = "number")]
    pub archived_at_ms: u64,
    /// Transcript entries.
    pub entries: u32,
}

/// User settings kept by the core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Settings {
    /// Archived Sessions kept per Repository (the newest).
    pub archive_keep_per_repository: u32,
    /// Archived Sessions older than this many days are pruned.
    pub archive_keep_days: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { archive_keep_per_repository: 50, archive_keep_days: 30 }
    }
}
