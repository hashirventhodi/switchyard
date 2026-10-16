//! Protocol types. Every type here is part of the JSON wire contract: events
//! and commands are internally tagged objects (`{"type": "...", ...}`) with
//! snake_case names, so an out-of-process adapter can speak them in any
//! language.

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version this crate defines. Adapters declare a semver range
/// they implement; the conformance runner rejects ranges this does not match.
pub const PROTOCOL_VERSION: Version = Version::new(0, 2, 0);

/// Something an adapter reports about the CLI it drives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The CLI has started. `permission_mode` is the mode the CLI actually
    /// started in (which may differ from the one requested), mapped to
    /// Switchyard's vocabulary; `cli_permission_mode` is the CLI's own name.
    SessionInit {
        external_session_id: String,
        permission_mode: PermissionMode,
        cli_permission_mode: String,
        cli: CliInfo,
        /// The model the CLI is using, when it says.
        #[serde(default)]
        model: Option<String>,
        /// Each MCP server the CLI was given, with its connection state at start.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mcp_servers: Vec<McpServerStatus>,
    },
    /// MCP servers whose connection state changed since last reported (a server
    /// still connecting at init settled, or one dropped). Servers not listed are
    /// unchanged.
    McpStatus {
        servers: Vec<McpServerStatus>,
    },
    AgentText {
        text: String,
    },
    ToolCallStarted {
        call_id: String,
        tool: String,
        input: Value,
    },
    ToolCallFinished {
        call_id: String,
        ok: bool,
        output: Option<String>,
    },
    /// The agent needs the user. Answered with [`Command::AnswerRequest`]
    /// carrying the same `request_id`.
    PermissionRequest {
        request_id: String,
        request: RequestDetail,
        reason: Option<String>,
    },
    /// The agent finished a turn and awaits the next prompt.
    TurnComplete,
    /// The agent declared its work complete.
    Done {
        summary: Option<String>,
    },
    Error {
        message: String,
        /// True when the CLI process can no longer continue.
        fatal: bool,
    },
    /// Usage on one Model: running totals for everything the Session has spent on
    /// `model`, superseding the previous report for the same Model. A CLI that uses
    /// several Models (its own subagent and background calls, a switch) sends one per
    /// Model, so each is priced on its own.
    TokenUsage {
        /// The Model these tokens were spent on. Absent (as in 0.1) means the Session's
        /// Model, whatever that is.
        #[serde(default)]
        model: Option<String>,
        /// Excluding cache reads and cache writes.
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        /// Tokens written to the prompt cache (cache creation); 0, and left out, when the
        /// CLI reports none.
        #[serde(default, skip_serializing_if = "is_zero")]
        cache_write_tokens: u64,
        /// Cost of these tokens as reported by the CLI, when it reports one.
        cost_usd: Option<f64>,
    },
    /// In Full auto, an action the CLI's own safety classifier refused.
    /// Never a Permission request.
    ClassifierDenial {
        tool: String,
        detail: String,
        reason: Option<String>,
    },
    /// The answer to [`Command::ListModels`]: the Models the CLI offers, in its order.
    ModelsListed {
        models: Vec<ListedModel>,
    },
    /// The answer to [`Command::ListModels`] from a CLI that cannot list its Models (or
    /// could not this time); `reason` says why, when known.
    CannotListModels {
        #[serde(default)]
        reason: Option<String>,
    },
    /// The main agent's own requests now run on `model`, a different Model from its last
    /// request; also sent for its first request when [`Event::SessionInit`] named no Model,
    /// which is how the host resolves an unreported Default (never from
    /// [`Event::TokenUsage`], whose Model may be a subagent's). Never sent for a subagent's, a compaction's or any other background call's
    /// Model (their usage still comes as [`Event::TokenUsage`] on that Model). An adapter that
    /// cannot tell the main agent's requests from the others never sends it, and says so in
    /// its manifest (`switch_model_live: false`).
    ///
    /// Only the user's switches change a Session's Model. The host takes this as the
    /// [`Command::SetModel`] it sent having taken only when `model` names that Model (its id,
    /// or the [`ListedModel::resolved_model`] an alias stands for); any other is the CLI
    /// moving on its own, which the host notes and never records as the Session's Model.
    ModelChanged {
        model: String,
    },
    /// The CLI will not run on `model`; `reason` is the CLI's, when it gives one. Either a
    /// [`Command::SetModel`] to `model` did not take (the CLI refused it, or the main agent's
    /// next request was still on the old Model), and the CLI carries on as it was; or, before
    /// the first request of a CLI resumed on `model` ([`Command::Resume`]) has run, the CLI
    /// cannot run on it (retired, or no access), and the host resumes it once more on Default.
    ModelChangeFailed {
        model: String,
        reason: Option<String>,
    },
}

impl Event {
    /// True when the event shows that a request of the agent's ran: it said or did
    /// something, or its turn ended. A host or adapter waiting on a relaunched CLI's first
    /// request (to confirm the Model it resumed on) counts from these.
    pub fn shows_a_request_ran(&self) -> bool {
        matches!(
            self,
            Event::AgentText { .. } | Event::ToolCallStarted { .. } | Event::TurnComplete
        )
    }
}

/// One Model a CLI offers: `id` is what the CLI takes to run on it, `display_name` what
/// the user reads, and `resolved_model` the full Model id `id` stands for when `id` is an
/// alias (Claude Code's `opus` for `claude-opus-5-5`), which is the name `session_init`,
/// `model_changed` and `token_usage` report it by. Absent when `id` is itself the full id or
/// the CLI does not say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedModel {
    pub id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
}

/// An MCP server's connection state, as the CLI reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub state: McpServerState,
    /// Why a failed server failed, when the CLI says.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerState {
    Connected,
    /// Still connecting.
    Pending,
    Failed,
    /// Needs the user to sign in (OAuth) before it connects.
    NeedsAuth,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Explore,
    AskFirst,
    AutoEdit,
    FullAuto,
    /// A bypass-everything mode; only used when the repo policy declares a sandbox.
    Bypass,
}

/// The five kinds of Permission request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestDetail {
    Command {
        command: String,
        cwd: Option<String>,
    },
    FileWrite {
        path: String,
        diff: Option<String>,
    },
    Network {
        domain: String,
        url: Option<String>,
    },
    McpTool {
        server: String,
        tool: String,
        input: Value,
    },
    Question {
        question: String,
        options: Vec<String>,
    },
}

/// Something the host tells an adapter to do to its CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    SendText {
        text: String,
    },
    AnswerRequest {
        request_id: String,
        verdict: Verdict,
    },
    Interrupt,
    /// Switch the running CLI to another Permission mode (the composer's mode segment).
    SetPermissionMode {
        permission_mode: PermissionMode,
    },
    /// Reload the CLI's conversation by its own session id, on `model` (none: Default, so
    /// the adapter passes no Model and the CLI or the user's own settings decide).
    Resume {
        external_session_id: String,
        #[serde(default)]
        model: Option<String>,
    },
    /// Ask the CLI which Models it offers. Answered by [`Event::ModelsListed`] or
    /// [`Event::CannotListModels`]; the host does the asking, so the adapter does no I/O.
    ListModels,
    /// Switch the running CLI to another Model, from the main agent's next request on (the
    /// request in flight is left alone). Answered by [`Event::ModelChanged`] once the main
    /// agent runs on it, or [`Event::ModelChangeFailed`]. Only sent to an adapter whose
    /// manifest has `switch_model_live`.
    SetModel {
        model: String,
    },
}

/// The user's (or policy's) answer to a Permission request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Verdict {
    /// Run once: approve this request without creating a Grant.
    AllowOnce,
    /// Approve and create a Grant. Grants live in Switchyard; whether the CLI
    /// is also told to remember the rule is the adapter's choice per CLI.
    AllowAlways,
    Deny {
        message: Option<String>,
    },
    /// Answer to a `question` request.
    Answer {
        text: String,
    },
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}
