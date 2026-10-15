//! Protocol types. Every type here is part of the JSON wire contract: events
//! and commands are internally tagged objects (`{"type": "...", ...}`) with
//! snake_case names, so an out-of-process adapter can speak them in any
//! language.

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version this crate defines. Adapters declare a semver range
/// they implement; the conformance runner rejects ranges this does not match.
pub const PROTOCOL_VERSION: Version = Version::new(0, 1, 0);

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
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        /// Cost as reported by the CLI, when it reports one.
        cost_usd: Option<f64>,
    },
    /// In Full auto, an action the CLI's own safety classifier refused.
    /// Never a Permission request.
    ClassifierDenial {
        tool: String,
        detail: String,
        reason: Option<String>,
    },
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
    Resume {
        external_session_id: String,
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
