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
