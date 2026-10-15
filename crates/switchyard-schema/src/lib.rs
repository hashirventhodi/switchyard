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

mod tasks;
pub use tasks::*;

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
    /// Switch a Session to another Permission mode (Ask first, Auto edit, Full auto),
    /// capped by the Repository's ceiling. Explore is only chosen at start.
    SetPermissionMode {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "permissionMode")]
        #[ts(rename = "permissionMode")]
        permission_mode: PermissionMode,
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
    /// Resolve one conflict hunk of a file git holds unmerged (a merge or rebase stopped on
    /// it) by keeping the branch's side or base's: the file is rewritten with that side in
    /// place of the hunk's markers, and once no markers remain git is told it is resolved.
    /// Refused while the worktree's Session is Running. Answered with
    /// [`Event::ConflictResolved`], then [`Event::ChangesRead`], [`Event::ConflictsRead`]
    /// and the worktree's facts (the dry-run merge re-run).
    ResolveConflict {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        path: String,
        #[serde(rename = "hunkId")]
        #[ts(rename = "hunkId")]
        hunk_id: String,
        side: ConflictSide,
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
    /// Acknowledge the Repository's pending policy request, named by its hash (refused when
    /// the policy has changed since): its allow rules, sandbox, default mode and worktree
    /// root apply from then on, until the request changes (ADR-0006).
    AcknowledgePolicy {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        hash: String,
    },
    /// List the MCP servers the Repository's `.mcp.json` declares and which the user has
    /// enabled. Answered with [`Event::McpServersChanged`].
    ReadMcpServers {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
    },
    /// Enable a declared MCP server for this Repository, for its definition as it stands
    /// (ADR-0006): passed to agents from the next spawn until its definition changes.
    EnableMcpServer {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        name: String,
    },
    DisableMcpServer {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        name: String,
    },
    /// Read the files an agent takes as instructions in a worktree (CLAUDE.md, AGENTS.md…)
    /// and Claude Code's memory for it. Read-only; answered with [`Event::ContextRead`].
    ReadContext {
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
    /// Detect the Forge's CLI (`gh`) and its sign-in again. Answered with
    /// [`Event::ForgeChecked`].
    CheckForge,
    /// Run a Forge poller tick now (pull requests and GitHub issues) and reset its back-off:
    /// the window was focused. Answered as any tick is, with the Forge events that changed.
    PollForgeNow,
    /// The window was shown or hidden. Hidden, the Forge poller polls every due item at its
    /// slowest cadence; shown, it polls at once and resets its back-off (as
    /// [`Command::PollForgeNow`]). Watching and agent events carry on either way.
    WindowVisibility { visible: bool },
    /// Push a worktree's branch and open a pull request against its Repository's base on
    /// the Forge. Refused when the worktree has uncommitted files, is behind its base, or
    /// the Forge's CLI is missing or signed out. Answered with [`Event::PullRequestOpened`]
    /// and [`Event::PullRequestChanged`]; the pull request is then polled until it closes
    /// or is Merged.
    OpenPullRequest {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
    },
    /// Remove every free (no session at work), clean, Merged worktree of a Repository, as
    /// the Merged cleanup does. Answered with [`Event::MergedPruned`].
    PruneMerged {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
    },
    /// Merge a worktree's branch into base in the Primary worktree, then clean the worktree
    /// up. Only for a Repository with no remote (ADR-0003). Answered with
    /// [`Event::BranchMerged`].
    LocalMerge {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
    },
    /// Read a Repository's Tasks from every TaskSource. Answered with [`Event::TasksChanged`].
    ReadTasks {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
    },
    /// Pull the Repository's GitHub Issues through the Forge onto the board (replacing what
    /// the last sync saw), then answer as [`Command::ReadTasks`]. Never writes to GitHub.
    /// When `gh` is missing or signed out the board keeps what it had and the refusal names
    /// the setup state.
    SyncTasks {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
    },
    /// Add a local Task to the Repository's To do column. `checklist` lists its sub-steps.
    /// Answered with [`Event::TaskCreated`] and [`Event::TasksChanged`].
    CreateTask {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        title: String,
        description: String,
        kind: TaskKind,
        checklist: Vec<String>,
    },
    /// Tick (or untick) checklist item `item` (0-based) of a Task. Kept in Switchyard's
    /// store only; never written to the task's tracker.
    TickTask {
        #[serde(rename = "taskId")]
        #[ts(rename = "taskId")]
        task_id: String,
        item: u32,
        done: bool,
    },
    /// Link a Session to a Task (`null`: unlink it). A Session links to at most one Task;
    /// replacing its link is refused unless `relink` says the user chose to. Linking moves
    /// a To do task to In progress. Answered with [`Event::TaskLinked`].
    LinkTask {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "taskId")]
        #[ts(rename = "taskId")]
        task_id: Option<String>,
        relink: bool,
    },
    /// "Assign to agent": start a Session for a Task, as [`Command::StartSession`],
    /// [`Command::StartSessionOnNewWorktree`] or [`Command::ReplaceSession`] would on
    /// `target`, and link it to the Task (which moves to In progress).
    StartTaskSession {
        #[serde(rename = "taskId")]
        #[ts(rename = "taskId")]
        task_id: String,
        target: TaskSessionTarget,
        agent: AgentKind,
        #[serde(rename = "permissionMode")]
        #[ts(rename = "permissionMode")]
        permission_mode: PermissionMode,
        prompt: String,
    },
    /// List a Repository's secrets by name, with grant flags and where each value would
    /// come from. Answered with [`Event::SecretsChanged`].
    ReadSecrets {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
    },
    /// Grant a secret to a Repository: from its next spawn on, every Session of the
    /// Repository (and the MCP server processes its CLI starts) has it in its environment.
    GrantSecret {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        name: String,
    },
    /// Revoke a secret's grant. Takes effect at each Session's next spawn; Sessions whose
    /// agent is running note it in their transcripts.
    RevokeSecret {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        name: String,
    },
    /// Name a secret for a Repository that neither its policy file nor its `.mcp.json`
    /// declares (kept as a local name, not granted).
    AddSecret {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        name: String,
    },
    /// Forget a locally added secret name (and its grant). Declared names stay.
    RemoveSecret {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        name: String,
    },
    /// Keep a secret's value in the OS keychain (never in the store or on disk). The value
    /// is never echoed back; answered with [`Event::SecretsChanged`] for the Repository.
    SetSecretValue {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        name: String,
        value: String,
    },
}

/// Every [`Command`]'s wire name (its `type` tag), for refusals raised before or apart from
/// the command value itself. A test checks the list against the serde tags.
pub mod command_name {
    pub const ATTACH: &str = "attach";
    pub const ADD_REPOSITORY: &str = "addRepository";
    pub const START_SESSION: &str = "startSession";
    pub const STOP_SESSION: &str = "stopSession";
    pub const SET_PERMISSION_MODE: &str = "setPermissionMode";
    pub const SEND_PROMPT: &str = "sendPrompt";
    pub const OPEN_SHELL: &str = "openShell";
    pub const WRITE_SHELL: &str = "writeShell";
    pub const RESIZE_SHELL: &str = "resizeShell";
    pub const CLOSE_SHELL: &str = "closeShell";
    pub const CLOSE_WORKTREE_SHELLS: &str = "closeWorktreeShells";
    pub const CREATE_WORKTREE: &str = "createWorktree";
    pub const START_SESSION_ON_NEW_WORKTREE: &str = "startSessionOnNewWorktree";
    pub const REMOVE_WORKTREE: &str = "removeWorktree";
    pub const REFRESH_WORKTREES: &str = "refreshWorktrees";
    pub const CHECK_AGENTS: &str = "checkAgents";
    pub const READ_CHANGES: &str = "readChanges";
    pub const DECIDE_FILE: &str = "decideFile";
    pub const COMMIT_ACCEPTED: &str = "commitAccepted";
    pub const DECIDE_HUNK: &str = "decideHunk";
    pub const COMMENT_HUNK: &str = "commentHunk";
    pub const SEND_FEEDBACK: &str = "sendFeedback";
    pub const REBASE_WORKTREE: &str = "rebaseWorktree";
    pub const RESOLVE_CONFLICT: &str = "resolveConflict";
    pub const ASK_AGENT_TO_REBASE: &str = "askAgentToRebase";
    pub const READ_HISTORY: &str = "readHistory";
    pub const ACKNOWLEDGE_POLICY: &str = "acknowledgePolicy";
    pub const READ_MCP_SERVERS: &str = "readMcpServers";
    pub const ENABLE_MCP_SERVER: &str = "enableMcpServer";
    pub const DISABLE_MCP_SERVER: &str = "disableMcpServer";
    pub const READ_CONTEXT: &str = "readContext";
    pub const ANSWER_REQUEST: &str = "answerRequest";
    pub const RESUME_SESSION: &str = "resumeSession";
    pub const REPLACE_SESSION: &str = "replaceSession";
    pub const READ_ARCHIVED_SESSION: &str = "readArchivedSession";
    pub const EXPORT_TRANSCRIPT: &str = "exportTranscript";
    pub const UPDATE_SETTINGS: &str = "updateSettings";
    pub const READ_POLICY: &str = "readPolicy";
    pub const REVOKE_GRANT: &str = "revokeGrant";
    pub const CHECK_FORGE: &str = "checkForge";
    pub const POLL_FORGE_NOW: &str = "pollForgeNow";
    pub const WINDOW_VISIBILITY: &str = "windowVisibility";
    pub const OPEN_PULL_REQUEST: &str = "openPullRequest";
    pub const PRUNE_MERGED: &str = "pruneMerged";
    pub const LOCAL_MERGE: &str = "localMerge";
    pub const READ_TASKS: &str = "readTasks";
    pub const SYNC_TASKS: &str = "syncTasks";
    pub const CREATE_TASK: &str = "createTask";
    pub const TICK_TASK: &str = "tickTask";
    pub const LINK_TASK: &str = "linkTask";
    pub const START_TASK_SESSION: &str = "startTaskSession";
    pub const READ_SECRETS: &str = "readSecrets";
    pub const GRANT_SECRET: &str = "grantSecret";
    pub const REVOKE_SECRET: &str = "revokeSecret";
    pub const ADD_SECRET: &str = "addSecret";
    pub const REMOVE_SECRET: &str = "removeSecret";
    pub const SET_SECRET_VALUE: &str = "setSecretValue";
    /// Every name, in declaration order.
    pub const ALL: &[&str] = &[ATTACH, ADD_REPOSITORY, START_SESSION, STOP_SESSION, SET_PERMISSION_MODE, SEND_PROMPT, OPEN_SHELL, WRITE_SHELL, RESIZE_SHELL, CLOSE_SHELL, CLOSE_WORKTREE_SHELLS, CREATE_WORKTREE, START_SESSION_ON_NEW_WORKTREE, REMOVE_WORKTREE, REFRESH_WORKTREES, CHECK_AGENTS, READ_CHANGES, DECIDE_FILE, COMMIT_ACCEPTED, DECIDE_HUNK, COMMENT_HUNK, SEND_FEEDBACK, REBASE_WORKTREE, RESOLVE_CONFLICT, ASK_AGENT_TO_REBASE, READ_HISTORY, ACKNOWLEDGE_POLICY, READ_MCP_SERVERS, ENABLE_MCP_SERVER, DISABLE_MCP_SERVER, READ_CONTEXT, ANSWER_REQUEST, RESUME_SESSION, REPLACE_SESSION, READ_ARCHIVED_SESSION, EXPORT_TRANSCRIPT, UPDATE_SETTINGS, READ_POLICY, REVOKE_GRANT, CHECK_FORGE, OPEN_PULL_REQUEST, PRUNE_MERGED, LOCAL_MERGE, READ_TASKS, SYNC_TASKS, CREATE_TASK, TICK_TASK, LINK_TASK, START_TASK_SESSION, READ_SECRETS, GRANT_SECRET, REVOKE_SECRET, ADD_SECRET, REMOVE_SECRET, SET_SECRET_VALUE,
        POLL_FORGE_NOW,
        WINDOW_VISIBILITY,
    ];
}

impl Command {
    /// This command's wire name (its `type` tag), as a `commandRefused` names it.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Attach { .. } => command_name::ATTACH,
            Command::AddRepository { .. } => command_name::ADD_REPOSITORY,
            Command::StartSession { .. } => command_name::START_SESSION,
            Command::StopSession { .. } => command_name::STOP_SESSION,
            Command::SetPermissionMode { .. } => command_name::SET_PERMISSION_MODE,
            Command::SendPrompt { .. } => command_name::SEND_PROMPT,
            Command::OpenShell { .. } => command_name::OPEN_SHELL,
            Command::WriteShell { .. } => command_name::WRITE_SHELL,
            Command::ResizeShell { .. } => command_name::RESIZE_SHELL,
            Command::CloseShell { .. } => command_name::CLOSE_SHELL,
            Command::CloseWorktreeShells { .. } => command_name::CLOSE_WORKTREE_SHELLS,
            Command::CreateWorktree { .. } => command_name::CREATE_WORKTREE,
            Command::StartSessionOnNewWorktree { .. } => command_name::START_SESSION_ON_NEW_WORKTREE,
            Command::RemoveWorktree { .. } => command_name::REMOVE_WORKTREE,
            Command::RefreshWorktrees { .. } => command_name::REFRESH_WORKTREES,
            Command::CheckAgents { .. } => command_name::CHECK_AGENTS,
            Command::ReadChanges { .. } => command_name::READ_CHANGES,
            Command::DecideFile { .. } => command_name::DECIDE_FILE,
            Command::CommitAccepted { .. } => command_name::COMMIT_ACCEPTED,
            Command::DecideHunk { .. } => command_name::DECIDE_HUNK,
            Command::CommentHunk { .. } => command_name::COMMENT_HUNK,
            Command::SendFeedback { .. } => command_name::SEND_FEEDBACK,
            Command::RebaseWorktree { .. } => command_name::REBASE_WORKTREE,
            Command::ResolveConflict { .. } => command_name::RESOLVE_CONFLICT,
            Command::AskAgentToRebase { .. } => command_name::ASK_AGENT_TO_REBASE,
            Command::ReadHistory { .. } => command_name::READ_HISTORY,
            Command::AcknowledgePolicy { .. } => command_name::ACKNOWLEDGE_POLICY,
            Command::ReadMcpServers { .. } => command_name::READ_MCP_SERVERS,
            Command::EnableMcpServer { .. } => command_name::ENABLE_MCP_SERVER,
            Command::DisableMcpServer { .. } => command_name::DISABLE_MCP_SERVER,
            Command::ReadContext { .. } => command_name::READ_CONTEXT,
            Command::AnswerRequest { .. } => command_name::ANSWER_REQUEST,
            Command::ResumeSession { .. } => command_name::RESUME_SESSION,
            Command::ReplaceSession { .. } => command_name::REPLACE_SESSION,
            Command::ReadArchivedSession { .. } => command_name::READ_ARCHIVED_SESSION,
            Command::ExportTranscript { .. } => command_name::EXPORT_TRANSCRIPT,
            Command::UpdateSettings { .. } => command_name::UPDATE_SETTINGS,
            Command::ReadPolicy { .. } => command_name::READ_POLICY,
            Command::RevokeGrant { .. } => command_name::REVOKE_GRANT,
            Command::CheckForge { .. } => command_name::CHECK_FORGE,
            Command::PollForgeNow { .. } => command_name::POLL_FORGE_NOW,
            Command::WindowVisibility { .. } => command_name::WINDOW_VISIBILITY,
            Command::OpenPullRequest { .. } => command_name::OPEN_PULL_REQUEST,
            Command::PruneMerged { .. } => command_name::PRUNE_MERGED,
            Command::LocalMerge { .. } => command_name::LOCAL_MERGE,
            Command::ReadTasks { .. } => command_name::READ_TASKS,
            Command::SyncTasks { .. } => command_name::SYNC_TASKS,
            Command::CreateTask { .. } => command_name::CREATE_TASK,
            Command::TickTask { .. } => command_name::TICK_TASK,
            Command::LinkTask { .. } => command_name::LINK_TASK,
            Command::StartTaskSession { .. } => command_name::START_TASK_SESSION,
            Command::ReadSecrets { .. } => command_name::READ_SECRETS,
            Command::GrantSecret { .. } => command_name::GRANT_SECRET,
            Command::RevokeSecret { .. } => command_name::REVOKE_SECRET,
            Command::AddSecret { .. } => command_name::ADD_SECRET,
            Command::RemoveSecret { .. } => command_name::REMOVE_SECRET,
            Command::SetSecretValue { .. } => command_name::SET_SECRET_VALUE,
        }
    }
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
    /// The Session now runs in another Permission mode (the user switched it).
    SessionModeChanged {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "permissionMode")]
        #[ts(rename = "permissionMode")]
        permission_mode: PermissionMode,
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
    /// Something in a worktree changed (on disk, or at the end of an agent turn), pushed
    /// beside its facts. A view showing heavy data for the worktree (its Changes, History or
    /// Context) re-reads it on the kinds it shows; nothing else is derived from this.
    WorktreeChanged {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        kinds: Vec<ChangeKind>,
    },
    /// Live updates are unavailable for a worktree: its watcher could not start or failed.
    /// Its facts are re-derived on a slow cadence until [`Event::WorktreeWatched`]. Replayed
    /// on attach for worktrees still in that state.
    WorktreeUnwatched {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        /// Why the watcher cannot run, for the person reading the notice.
        reason: String,
    },
    /// A worktree's watcher works again: live updates resume (the all-clear for
    /// [`Event::WorktreeUnwatched`]).
    WorktreeWatched {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
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
        /// Accepted files left out because they changed since you decided (their Accept is
        /// a Stale decision): re-confirm them to commit them.
        #[serde(rename = "skippedStale")]
        #[ts(rename = "skippedStale")]
        skipped_stale: Vec<String>,
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
    /// The worktree's conflicts with their hunks, read with [`Event::ChangesRead`]:
    /// files git holds unmerged, then files a merge of `base` would conflict on. Replaces
    /// earlier ones.
    ConflictsRead {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        base: String,
        files: Vec<ConflictFile>,
    },
    /// One conflict hunk was resolved by keeping `side`; `remaining` hunks of the file
    /// still conflict.
    ConflictResolved {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        path: String,
        side: ConflictSide,
        remaining: u32,
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
    /// The Repository's declared MCP servers and whether each is enabled (replace-whole).
    McpServersChanged {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        servers: Vec<DeclaredMcpServer>,
        /// Why `.mcp.json` could not be read, when it could not.
        error: Option<String>,
    },
    /// Rebase on base stopped on conflicts and was left stopped: they are real conflicts in
    /// the worktree now, to resolve (Keep ours / Keep base, or the agent), after which
    /// Rebase again continues it.
    RebaseStopped {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        base: String,
        conflicts: Vec<String>,
    },
    /// The worktree's context files as they are on disk (replaces earlier ones for it).
    ContextRead {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        files: Vec<ContextFile>,
    },
    /// A Repository's Policy as it stands: sent after [`Command::ReadPolicy`], and whenever
    /// a local Grant is added or revoked. Replaces earlier policy for that Repository.
    PolicyChanged {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        policy: Policy,
    },
    /// Whether pull requests can be opened: the Forge's CLI is installed and signed in.
    /// Sent on attach and after [`Command::CheckForge`] (or when opening a pull request
    /// detects a change); replaces the earlier result.
    ForgeChecked { forge: ForgeSetup },
    /// The pull request for a worktree's branch as the Forge reports it now (`null`: there
    /// is none). Replaces the earlier one for that path.
    PullRequestChanged {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        #[serde(rename = "pullRequest")]
        #[ts(rename = "pullRequest")]
        pull_request: Option<PullRequest>,
        /// When the Forge was last asked about it, in milliseconds since the Unix epoch:
        /// the renderer shows "checked N s ago" from it.
        #[serde(rename = "checkedAtMs")]
        #[ts(rename = "checkedAtMs", type = "number")]
        checked_at_ms: u64,
    },
    /// A pull request was opened from Switchyard for the worktree's branch.
    PullRequestOpened {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        #[serde(rename = "pullRequest")]
        #[ts(rename = "pullRequest")]
        pull_request: PullRequest,
    },
    /// A worktree's branch is in base: the Forge reported its pull request Merged, or a
    /// Local merge finished. `removed` says whether the worktree was cleaned up (removed,
    /// its shells closed, its Session archived); when not, `kept` says why.
    BranchMerged {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
        branch: String,
        base: String,
        /// The pull request's number; `null` for a Local merge.
        #[serde(rename = "pullRequest")]
        #[ts(rename = "pullRequest")]
        pull_request: Option<u32>,
        removed: bool,
        kept: Option<String>,
    },
    /// "Prune merged" finished: the branches whose worktrees were removed.
    MergedPruned {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        branches: Vec<String>,
    },
    /// Every Task of a Repository, from every TaskSource (replaces the earlier list), and
    /// the house rules a session's prompt for one of them carries.
    TasksChanged {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        tasks: Vec<Task>,
        #[serde(rename = "houseRules")]
        #[ts(rename = "houseRules")]
        house_rules: String,
        /// When the GitHub issues were last read from the Forge, in milliseconds since the
        /// Unix epoch; `null` when they never were (no remote, or never synced).
        #[serde(rename = "checkedAtMs")]
        #[ts(rename = "checkedAtMs", type = "number | null")]
        checked_at_ms: Option<u64>,
    },
    /// The user added a local Task.
    TaskCreated { task: Task },
    /// The Task a Session is linked to (`null`: none), replacing the earlier link. Replayed
    /// on attach for every linked Session, live or archived.
    TaskLinked {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "taskId")]
        #[ts(rename = "taskId")]
        task_id: Option<String>,
    },
    /// A Repository's secrets as they stand: sent after [`Command::ReadSecrets`] and after
    /// every change to them. Names and flags only, never values.
    SecretsChanged {
        #[serde(rename = "repositoryId")]
        #[ts(rename = "repositoryId")]
        repository_id: String,
        secrets: Vec<Secret>,
    },
}

/// A secret a Repository's Sessions may be given, by name. Its value lives in the OS
/// keychain or Switchyard's environment, never in the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Secret {
    /// The environment variable it is injected as.
    pub name: String,
    pub granted: bool,
    pub origin: SecretOrigin,
    /// Where its value would come from at the next spawn.
    pub source: SecretSource,
}

/// Why a Repository lists a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SecretOrigin {
    /// Named in `.switchyard/policy.yml` `secrets:`.
    Policy,
    /// Referenced as `${NAME}` by a server in the Repository's `.mcp.json`.
    Mcp,
    /// Added by the user in Switchyard.
    Local,
}

/// Where a secret's value comes from: the OS keychain wins over the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SecretSource {
    Keychain,
    Environment,
    /// Neither has it: a granted secret is then simply absent from the environment.
    Missing,
}

/// Whether the Forge can be used from this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ForgeSetup {
    /// Display name, e.g. "GitHub".
    pub name: String,
    pub state: ForgeSetupState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum ForgeSetupState {
    /// Installed and signed in.
    Ready { version: String },
    /// The CLI is not installed.
    Missing {
        #[serde(rename = "installHint")]
        #[ts(rename = "installHint")]
        install_hint: String,
        link: Link,
    },
    /// Installed but not signed in (or its sign-in no longer works).
    SignedOut {
        #[serde(rename = "loginHint")]
        #[ts(rename = "loginHint")]
        login_hint: String,
    },
}

/// A pull request on the Forge, as last polled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PullRequest {
    pub number: u32,
    pub url: String,
    pub state: PullRequestState,
    pub draft: bool,
    pub checks: ChecksState,
    /// Every check the Forge reports on the pull request's head, in the Forge's order.
    #[serde(default)]
    pub check_runs: Vec<Check>,
    pub review: ReviewState,
}

/// One check on a pull request: a CI job or a commit status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    /// Where the Forge shows the check's details, when it says.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CheckStatus {
    /// Queued or expected, not started.
    Pending,
    Running,
    Passing,
    Failing,
    /// Skipped, neutral or stale: finished without a verdict.
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PullRequestState {
    Open,
    /// Closed without merging.
    Closed,
    /// The Forge reports the branch is in base.
    Merged,
}

/// The pull request's checks, summed up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChecksState {
    /// No checks reported.
    None,
    Running,
    Failing,
    Passing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReviewState {
    /// No review decision (none required, or none given yet).
    None,
    Required,
    ChangesRequested,
    Approved,
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
    /// What the committed policy file asks for that would expand privilege and the user has
    /// not acknowledged (ADR-0006): none of it applies until then.
    #[serde(default)]
    pub request: Option<PolicyRequest>,
}

/// The privilege-expanding parts of a Repository's committed policy, awaiting the user's
/// acknowledgement (ADR-0006).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PolicyRequest {
    /// What [`Command::AcknowledgePolicy`] must name: a hash of exactly these parts.
    pub hash: String,
    /// The committed allow rules (not applied until acknowledged).
    pub allow: Vec<Grant>,
    /// The policy declares a sandbox (Full auto would bypass the CLI's checks).
    pub sandbox: bool,
    /// A default mode above Ask first (Ask first is used until acknowledged).
    pub default_mode: Option<PermissionMode>,
    /// A worktree root of the repository's own (Switchyard's is used until acknowledged).
    pub worktrees_root: Option<String>,
    /// What Switchyard detected that supports a sandbox claim (a container or VM it runs
    /// in); none when it cannot verify one.
    pub sandbox_evidence: Option<String>,
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
    /// The file's Decision is a Stale decision: it was made on content that has since
    /// changed, so it is shown but does not count until re-confirmed.
    pub stale: bool,
    pub hunks: Vec<Hunk>,
    /// Hunk Decisions on hunks no longer in the diff (the hunk changed since you decided):
    /// kept to show, never counted.
    pub stale_hunks: Vec<StaleHunk>,
}

/// A hunk Decision whose hunk has since changed (a Stale decision). With `decision` Reject
/// and `sent`, the agent revised the hunk after receiving the feedback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StaleHunk {
    /// The id the hunk had when decided.
    pub id: String,
    /// The hunk's `@@` line when decided.
    pub header: String,
    pub decision: Decision,
    pub comment: Option<String>,
    /// The Hunk feedback had been sent to the agent.
    pub sent: bool,
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

/// A file that conflicts, with its conflict hunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConflictFile {
    pub path: String,
    pub kind: ConflictKind,
    /// Empty when the conflict has no text hunks (a binary file, or one side deleted it).
    pub hunks: Vec<ConflictHunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ConflictKind {
    /// Git holds the file unmerged: a merge or rebase stopped here and the working tree has
    /// conflict markers. Keep ours / Keep base resolve it.
    Unmerged,
    /// Only a dry-run merge of base conflicts here; the working tree is untouched. Resolved
    /// by rebasing (or asking the agent to).
    Base,
}

/// One conflicting region: the branch's lines and base's lines for the same place, with up
/// to three unconflicted lines either side. `id` hashes the path, the region's place among
/// the file's conflicts and both sides, so it changes when the conflict does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConflictHunk {
    pub id: String,
    pub before: Vec<String>,
    /// The branch's lines.
    pub ours: Vec<String>,
    /// Base's lines.
    pub base: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ConflictSide {
    /// The branch's lines.
    Ours,
    /// Base's lines.
    Base,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// An MCP server the Repository's `.mcp.json` declares (ADR-0006).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeclaredMcpServer {
    pub name: String,
    /// "stdio · npx", "http · host": never arguments, environment or headers.
    pub transport: String,
    /// Enabled for exactly this definition: passed to agents.
    pub enabled: bool,
    /// Enabled once, but its definition has changed since: asks again.
    pub changed: bool,
}

/// A file an agent reads as context: instructions in the worktree, or memory the CLI
/// keeps for it. Shown read-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextFile {
    /// As the user knows it: `CLAUDE.md`, `.claude/CLAUDE.md`, `MEMORY.md`.
    pub name: String,
    /// Where it is on disk.
    pub path: String,
    pub kind: ContextKind,
    pub text: String,
    /// Only the start of a long file is sent.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ContextKind {
    /// In the worktree; committed with the repository (or kept local beside it).
    Instructions,
    /// What the agent CLI remembers for this worktree, outside the repository.
    Memory,
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
    /// Files the Changes view would list: working-tree changes against the index (renames
    /// paired), untracked files, unmerged files and files in `conflicts`.
    pub changed_files: u32,
    /// Commits the History view would list: those ahead of the base branch, up to its limit.
    pub commits: u32,
}

/// What kind of thing changed in a worktree (see [`Event::WorktreeChanged`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChangeKind {
    /// Files in the working tree that git does not ignore.
    Files,
    /// What git records: HEAD, the index, refs, the HEAD log, worktrees.
    Git,
    /// The Repository's configuration files.
    Config,
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
    /// The git remote branches are pushed to (`origin` when there is one), or absent when
    /// the Repository has no remote: then Local merge replaces Open pull request.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub remote: Option<String>,
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
    /// OpenAI Codex, driven through `codex app-server`.
    Codex,
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
    /// Set when the CLI started in another Permission mode than the one asked for (for
    /// example Full auto unavailable, so the CLI fell back to Ask first): shown on the
    /// summary strip and the session card.
    #[serde(default)]
    pub mode_warning: Option<String>,
    /// Actions the CLI's own classifier refused in Full auto ("3 denied by classifier").
    #[serde(default)]
    pub classifier_denials: u32,
    /// The MCP servers the CLI was given (the repository's `.mcp.json`, and only those),
    /// each with its connection state as the CLI last reported it. Shown in Context.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerFact>,
}

/// One MCP server a Session's CLI was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct McpServerFact {
    pub name: String,
    /// The transport and what it reaches, without arguments, environment or headers
    /// (`stdio · npx`, `http · mcp.example.com`).
    pub transport: String,
    pub state: McpState,
    /// Why a failed server failed, when the CLI says.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum McpState {
    /// Not yet reported by the CLI, or still connecting.
    Pending,
    Connected,
    /// Not connected: it failed to start or to connect.
    Failed,
    /// Needs the user to sign in before it connects.
    NeedsAuth,
    Disabled,
}

/// A Session's running spend. Cost is either reported by the CLI or, for a CLI that
/// reports tokens only, estimated from the price table when read (shown with a tilde);
/// the two are never mixed into one figure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Spend {
    /// Input tokens, not counting cache reads or writes.
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    #[ts(type = "number")]
    pub cache_read_tokens: u64,
    /// Tokens written to the prompt cache (priced apart from input).
    #[serde(default)]
    #[ts(type = "number")]
    pub cache_write_tokens: u64,
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
