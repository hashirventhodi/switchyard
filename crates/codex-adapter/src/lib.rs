//! The Codex adapter: maps OpenAI Codex's `codex app-server` (JSON-RPC over stdio, one JSON
//! message per line, no `"jsonrpc"` field) onto the Switchyard adapter protocol. Like every
//! adapter it never touches a process: the host writes the raw messages it returns (from
//! [`Adapter::command`] and [`Adapter::replies`]) to the app-server's stdin and feeds it
//! each line of its stdout.
//!
//! A session runs as one Codex thread:
//!
//! 1. The host's first command (`send_text`, or `resume` after a relaunch) opens the
//!    handshake: `initialize`; on its result, `initialized` and `config/read` for the
//!    worktree (to learn which MCP servers Codex would load on its own), then
//!    `thread/start` (or `thread/resume` by thread id) with the worktree as `cwd`, the
//!    permission mode as approval policy + sandbox ([`mode`]) and the config overrides
//!    ([`thread_config`]). Prompts sent meanwhile are queued.
//! 2. The thread's result is `session_init` (the thread id is the external session id; the
//!    mode is read back from the policy Codex actually applied). Queued prompts become the
//!    first `turn/start`.
//! 3. Items map to agent text and tool calls; server requests (approvals, questions, MCP
//!    tool approvals) to Permission requests, answered with the same JSON-RPC id; token
//!    usage to running totals; `turn/completed` to `turn_complete`.
//!
//! Types are a hand-written subset of the schema `codex app-server generate-json-schema`
//! prints for codex-cli 0.144.1, read leniently (unknown fields ignored), since real
//! output carries fields the published schema does not.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use adapter_protocol::{
    Adapter, AdapterError, AdapterManifest, CliInfo, Command, Event, McpServerState,
    McpServerStatus, PermissionMode, RequestDetail, Verdict,
};
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub mod mode;

/// The adapter's name, and the CLI name it reports in `session_init`.
pub const NAME: &str = "codex";

/// What this adapter declares about itself.
pub fn manifest() -> AdapterManifest {
    AdapterManifest {
        name: NAME.into(),
        implements: VersionReq::parse("^0.2").expect("valid range"),
        // Not yet: a host never sends this adapter set_model, and it never sends model_changed.
        switch_model_live: false,
    }
}

/// What the host starts a session with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Launch {
    /// The worktree: the thread's working root.
    pub cwd: String,
    /// The mode to ask Codex for (bypass only when the repository declares a sandbox).
    pub permission_mode: PermissionMode,
    /// Per-spawn config overrides, as config.toml dotted keys to JSON values, passed as the
    /// thread's `config` (never written to Codex's own files). The MCP servers Switchyard
    /// gives this session are defined here (`mcp_servers.<name>.command`, ...).
    #[serde(default)]
    pub config: Map<String, Value>,
    /// The names of the MCP servers `config` defines: the only ones the session may use.
    /// Every other server Codex would load (from its own config files) is switched off for
    /// this thread, as Claude Code's strict MCP mode does.
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    /// The resolved Policy's MCP tool Grants and denials, translated into the thread's
    /// config for the servers in `mcp_servers` ([`McpRule`]).
    #[serde(default)]
    pub mcp_rules: Vec<McpRule>,
}

/// A Grant (`allow`) or denial for one MCP tool, `tool` `*` meaning every tool of `server`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpRule {
    pub server: String,
    pub tool: String,
    pub allow: bool,
}

/// What an outgoing request was, to route its response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Initialize,
    ConfigRead,
    Thread,
    TurnStart,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Nothing written yet.
    New,
    /// Handshake in flight: prompts are queued.
    Starting,
    /// The thread exists.
    Ready,
    /// Starting the thread failed; nothing more is sent.
    Failed,
}

/// A server request awaiting the host's verdict.
#[derive(Debug, Clone)]
struct Pending {
    id: Value,
    kind: PendingKind,
}

#[derive(Debug, Clone)]
enum PendingKind {
    /// v2 command or file-change approval: `{decision: accept|decline}`.
    Decision,
    /// v1 `execCommandApproval` / `applyPatchApproval`: `{decision: approved|denied}`.
    LegacyDecision,
    /// `item/tool/requestUserInput`: answers by question id.
    Questions(Vec<String>),
    /// `mcpServer/elicitation/request` asking to approve an MCP tool call.
    McpApproval,
    /// Any other elicitation, asked as an Accept/Decline question.
    Elicitation,
    /// `item/permissions/requestApproval`: grant (echo) or refuse the requested profile.
    Permissions(Value),
}

pub struct CodexAdapter {
    launch: Launch,
    phase: Phase,
    next_id: u64,
    calls: HashMap<u64, Call>,
    replies: Vec<String>,
    /// Prompts sent before the thread existed.
    queued: Vec<String>,
    /// The thread to resume instead of starting one.
    resume: Option<String>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    /// The thread's Model, from `thread/start` or `thread/resume`: what its usage is spent on.
    model: Option<String>,
    /// From `initialize`'s user agent: the running CLI's version.
    cli_version: Option<String>,
    /// A mode switch to carry on the next `turn/start`.
    mode_override: Option<PermissionMode>,
    pending: HashMap<String, Pending>,
    /// Paths and diffs of each fileChange item, for its approval request.
    file_changes: HashMap<String, (String, Option<String>)>,
    /// Items reported as started tool calls.
    started: BTreeSet<String>,
    /// Turns whose failure was already reported by an `error` notification.
    reported: BTreeSet<String>,
    /// Each MCP server's last reported state.
    mcp: BTreeMap<String, McpServerState>,
}

impl CodexAdapter {
    pub fn new(launch: Launch) -> Self {
        CodexAdapter {
            launch,
            phase: Phase::New,
            next_id: 1,
            calls: HashMap::new(),
            replies: Vec::new(),
            queued: Vec::new(),
            model: None,
            resume: None,
            thread_id: None,
            turn_id: None,
            cli_version: None,
            mode_override: None,
            pending: HashMap::new(),
            file_changes: HashMap::new(),
            started: BTreeSet::new(),
            reported: BTreeSet::new(),
            mcp: BTreeMap::new(),
        }
    }

    /// Every server Switchyard gave the session, at thread start: still connecting unless
    /// Codex already said otherwise.
    fn mcp_at_init(&mut self) -> Vec<McpServerStatus> {
        let mut statuses = Vec::new();
        for name in &self.launch.mcp_servers {
            let state = *self
                .mcp
                .entry(name.clone())
                .or_insert(McpServerState::Pending);
            statuses.push(McpServerStatus {
                name: name.clone(),
                state,
                error: None,
            });
        }
        statuses
    }

    /// `mcpServer/startupStatus/updated`: one `mcp_status` when the state changed.
    fn mcp_startup(&mut self, params: &Value) -> Vec<Event> {
        let Some(name) = str_at(params, "/name") else {
            return vec![];
        };
        let state = match str_at(params, "/status").as_deref() {
            Some("starting") => McpServerState::Pending,
            Some("ready") => McpServerState::Connected,
            Some("failed") | Some("cancelled") => McpServerState::Failed,
            _ => return vec![],
        };
        if self.mcp.insert(name.clone(), state) == Some(state) {
            return vec![];
        }
        let error = str_at(params, "/error").or_else(|| str_at(params, "/failureReason"));
        let error = if state == McpServerState::Failed {
            error
        } else {
            None
        };
        vec![Event::McpStatus {
            servers: vec![McpServerStatus { name, state, error }],
        }]
    }

    fn request(&mut self, call: Call, method: &str, params: Value) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.calls.insert(id, call);
        json!({ "method": method, "id": id, "params": params }).to_string()
    }

    fn initialize(&mut self) -> String {
        self.phase = Phase::Starting;
        let params = json!({
            "clientInfo": { "name": "switchyard", "title": "Switchyard", "version": "0.1.0" },
            "capabilities": null,
        });
        self.request(Call::Initialize, "initialize", params)
    }

    /// `thread/start`, or `thread/resume` by id, with the mode and the config overrides.
    fn open_thread(&mut self, config: Map<String, Value>) -> String {
        let (approval, sandbox) = mode::thread_policy(self.launch.permission_mode);
        let mut params = Map::new();
        if let Some(thread_id) = &self.resume {
            params.insert("threadId".into(), json!(thread_id));
        }
        params.insert("cwd".into(), json!(self.launch.cwd));
        params.insert("approvalPolicy".into(), json!(approval));
        // Prompts route to Switchyard, never to Codex's own reviewer.
        params.insert("approvalsReviewer".into(), json!("user"));
        params.insert("sandbox".into(), json!(sandbox));
        if !config.is_empty() {
            params.insert("config".into(), Value::Object(config));
        }
        let method = if self.resume.is_some() {
            "thread/resume"
        } else {
            "thread/start"
        };
        self.request(Call::Thread, method, Value::Object(params))
    }

    fn turn_start(&mut self, texts: Vec<String>) -> String {
        let thread_id = self.thread_id.clone().unwrap_or_default();
        let input: Vec<Value> = texts
            .into_iter()
            .map(|text| json!({ "type": "text", "text": text, "text_elements": [] }))
            .collect();
        let mut params = Map::new();
        params.insert("threadId".into(), json!(thread_id));
        params.insert("input".into(), Value::Array(input));
        // A switched mode rides on the next turn; Codex keeps it for later turns.
        if let Some(mode) = self.mode_override.take() {
            let (approval, sandbox) = mode::turn_policy(mode);
            params.insert("approvalPolicy".into(), approval);
            params.insert("sandboxPolicy".into(), sandbox);
        }
        self.request(Call::TurnStart, "turn/start", Value::Object(params))
    }

    fn on_response(&mut self, id: u64, message: &Value) -> Vec<Event> {
        let call = self.calls.remove(&id).unwrap_or(Call::Other);
        let error = message.get("error");
        match call {
            Call::Initialize => {
                if let Some(error) = error {
                    return self.fail(format!(
                        "Codex did not initialise: {}",
                        error_message(error)
                    ));
                }
                self.cli_version = message
                    .pointer("/result/userAgent")
                    .and_then(Value::as_str)
                    .and_then(version_from_user_agent);
                self.replies
                    .push(json!({ "method": "initialized" }).to_string());
                let read = self.request(
                    Call::ConfigRead,
                    "config/read",
                    json!({ "includeLayers": false, "cwd": self.launch.cwd }),
                );
                self.replies.push(read);
                vec![]
            }
            Call::ConfigRead => {
                // Without the effective config (an error), nothing is switched off.
                let codex_servers: BTreeSet<String> = message
                    .pointer("/result/config/mcp_servers")
                    .and_then(Value::as_object)
                    .map(|servers| servers.keys().cloned().collect())
                    .unwrap_or_default();
                let config = thread_config(&self.launch, &codex_servers);
                let start = self.open_thread(config);
                self.replies.push(start);
                vec![]
            }
            Call::Thread => {
                let Some(result) = message.get("result").filter(|_| error.is_none()) else {
                    let what = if self.resume.is_some() {
                        "resume the thread"
                    } else {
                        "start a thread"
                    };
                    return self.fail(format!(
                        "Codex could not {what}: {}",
                        error_message(error.unwrap_or(&Value::Null))
                    ));
                };
                let thread_id = str_at(result, "/thread/id").unwrap_or_default();
                self.model = str_at(result, "/model");
                self.thread_id = Some(thread_id.clone());
                self.phase = Phase::Ready;
                let (permission_mode, cli_permission_mode) =
                    mode::read_back(result.get("approvalPolicy"), result.get("sandbox"), self.launch.permission_mode);
                let version = self
                    .cli_version
                    .clone()
                    .or_else(|| str_at(result, "/thread/cliVersion"))
                    .unwrap_or_default();
                let init = Event::SessionInit {
                    external_session_id: thread_id,
                    permission_mode,
                    cli_permission_mode,
                    cli: CliInfo {
                        name: NAME.into(),
                        version,
                    },
                    model: self.model.clone(),
                    mcp_servers: self.mcp_at_init(),
                };
                if !self.queued.is_empty() {
                    let texts = std::mem::take(&mut self.queued);
                    let turn = self.turn_start(texts);
                    self.replies.push(turn);
                }
                vec![init]
            }
            Call::TurnStart => match error {
                // The prompt never became a turn: report it and hand the session back.
                Some(error) => vec![
                    Event::Error {
                        message: format!("Codex did not start the turn: {}", error_message(error)),
                        fatal: false,
                    },
                    Event::TurnComplete,
                ],
                None => {
                    if let Some(turn_id) = str_at(message, "/result/turn/id") {
                        self.turn_id = Some(turn_id);
                    }
                    vec![]
                }
            },
            Call::Other => vec![],
        }
    }

    fn fail(&mut self, message: String) -> Vec<Event> {
        self.phase = Phase::Failed;
        self.queued.clear();
        vec![Event::Error {
            message,
            fatal: true,
        }]
    }

    fn on_notification(&mut self, method: &str, params: &Value) -> Vec<Event> {
        match method {
            "turn/started" => {
                self.turn_id = str_at(params, "/turn/id");
                vec![]
            }
            "turn/completed" => {
                let turn_id = str_at(params, "/turn/id").unwrap_or_default();
                self.turn_id = None;
                let mut events = Vec::new();
                if str_at(params, "/turn/status").as_deref() == Some("failed")
                    && !self.reported.contains(&turn_id)
                {
                    let message = str_at(params, "/turn/error/message")
                        .unwrap_or_else(|| "The turn failed".into());
                    events.push(Event::Error {
                        message,
                        fatal: false,
                    });
                }
                events.push(Event::TurnComplete);
                events
            }
            "error" => {
                if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
                    return vec![];
                }
                if let Some(turn_id) = str_at(params, "/turnId") {
                    self.reported.insert(turn_id);
                }
                let message = str_at(params, "/error/message")
                    .unwrap_or_else(|| "Codex reported an error".into());
                vec![Event::Error {
                    message,
                    fatal: false,
                }]
            }
            "mcpServer/startupStatus/updated" => self.mcp_startup(params),
            "item/started" => self.item_started(params.get("item").unwrap_or(&Value::Null)),
            "item/completed" => self.item_completed(params.get("item").unwrap_or(&Value::Null)),
            "thread/tokenUsage/updated" => {
                let total = params.pointer("/tokenUsage/total").unwrap_or(&Value::Null);
                let count = |name: &str| total.get(name).and_then(Value::as_u64).unwrap_or(0);
                let cached = count("cachedInputTokens");
                vec![Event::TokenUsage {
                    model: self.model.clone(),
                    // The protocol's input excludes cache reads; Codex's includes them.
                    input_tokens: count("inputTokens").saturating_sub(cached),
                    output_tokens: count("outputTokens"),
                    cache_read_tokens: cached,
                    // OpenAI caches prompts without a write charge or count.
                    cache_write_tokens: 0,
                    cost_usd: None,
                }]
            }
            "serverRequest/resolved" => {
                if let Some(id) = params.get("requestId") {
                    self.pending.remove(&request_key(id));
                }
                vec![]
            }
            _ => vec![],
        }
    }

    fn item_started(&mut self, item: &Value) -> Vec<Event> {
        let Some(id) = str_at(item, "/id") else {
            return vec![];
        };
        if let Some("fileChange") = item.get("type").and_then(Value::as_str) {
            let changes = item
                .get("changes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let paths: Vec<String> = changes.iter().filter_map(|c| str_at(c, "/path")).collect();
            let diffs: Vec<String> = changes.iter().filter_map(|c| str_at(c, "/diff")).collect();
            let diff = (!diffs.is_empty()).then(|| diffs.join("\n"));
            self.file_changes
                .insert(id.clone(), (paths.join(", "), diff));
        }
        match tool_call(item) {
            Some((tool, input)) => {
                self.started.insert(id.clone());
                vec![Event::ToolCallStarted {
                    call_id: id,
                    tool,
                    input,
                }]
            }
            None => vec![],
        }
    }

    fn item_completed(&mut self, item: &Value) -> Vec<Event> {
        let Some(id) = str_at(item, "/id") else {
            return vec![];
        };
        let status = str_at(item, "/status");
        let completed = status.as_deref() == Some("completed");
        let finished = match item.get("type").and_then(Value::as_str) {
            Some("agentMessage") => {
                let text = str_at(item, "/text").unwrap_or_default();
                return if text.is_empty() {
                    vec![]
                } else {
                    vec![Event::AgentText { text }]
                };
            }
            Some("commandExecution") => {
                let exit_ok = item
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .is_none_or(|code| code == 0);
                (completed && exit_ok, str_at(item, "/aggregatedOutput"))
            }
            Some("fileChange") => {
                self.file_changes.remove(&id);
                (completed, None)
            }
            Some("mcpToolCall") => {
                let output = str_at(item, "/error/message").or_else(|| {
                    let content = item.pointer("/result/content").and_then(Value::as_array)?;
                    let texts: Vec<String> =
                        content.iter().filter_map(|c| str_at(c, "/text")).collect();
                    (!texts.is_empty()).then(|| texts.join("\n"))
                });
                (completed, output)
            }
            Some("webSearch") => (true, None),
            _ => return vec![],
        };
        let mut events = Vec::new();
        if !self.started.remove(&id) {
            if let Some((tool, input)) = tool_call(item) {
                events.push(Event::ToolCallStarted {
                    call_id: id.clone(),
                    tool,
                    input,
                });
            }
        }
        events.push(Event::ToolCallFinished {
            call_id: id,
            ok: finished.0,
            output: finished.1,
        });
        events
    }

    fn on_server_request(&mut self, id: &Value, method: &str, params: &Value) -> Vec<Event> {
        let reason = str_at(params, "/reason");
        let (kind, request, reason) = match method {
            "item/commandExecution/requestApproval" => {
                let request = match str_at(params, "/networkApprovalContext/host") {
                    Some(domain) => RequestDetail::Network { domain, url: None },
                    None => RequestDetail::Command {
                        command: str_at(params, "/command").unwrap_or_default(),
                        cwd: str_at(params, "/cwd"),
                    },
                };
                (PendingKind::Decision, request, reason)
            }
            "item/fileChange/requestApproval" => {
                let item_id = str_at(params, "/itemId").unwrap_or_default();
                let (path, diff) =
                    self.file_changes.get(&item_id).cloned().unwrap_or_else(|| {
                        (str_at(params, "/grantRoot").unwrap_or_default(), None)
                    });
                (
                    PendingKind::Decision,
                    RequestDetail::FileWrite { path, diff },
                    reason,
                )
            }
            "item/tool/requestUserInput" => {
                let questions = params
                    .get("questions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let ids = questions.iter().filter_map(|q| str_at(q, "/id")).collect();
                let (question, options, header) = match questions.as_slice() {
                    [one] => (
                        str_at(one, "/question").unwrap_or_default(),
                        one.get("options")
                            .and_then(Value::as_array)
                            .map(|o| o.iter().filter_map(|o| str_at(o, "/label")).collect())
                            .unwrap_or_default(),
                        str_at(one, "/header"),
                    ),
                    // Several questions: asked as one, answered with the same text.
                    many => (
                        many.iter()
                            .filter_map(|q| str_at(q, "/question"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        vec![],
                        None,
                    ),
                };
                (
                    PendingKind::Questions(ids),
                    RequestDetail::Question { question, options },
                    header,
                )
            }
            "mcpServer/elicitation/request" => {
                let message = str_at(params, "/message");
                if str_at(params, "/_meta/codex_approval_kind").as_deref() == Some("mcp_tool_call")
                {
                    let request = RequestDetail::McpTool {
                        server: str_at(params, "/serverName").unwrap_or_default(),
                        tool: str_at(params, "/_meta/tool_name").unwrap_or_default(),
                        input: params
                            .pointer("/_meta/tool_params")
                            .cloned()
                            .unwrap_or(Value::Null),
                    };
                    (PendingKind::McpApproval, request, message)
                } else {
                    let server = str_at(params, "/serverName").unwrap_or_default();
                    let mut question =
                        message.unwrap_or_else(|| format!("{server} asks for input"));
                    if let Some(url) = str_at(params, "/url") {
                        question = format!("{question}\n{url}");
                    }
                    let options = vec!["Accept".to_string(), "Decline".to_string()];
                    (
                        PendingKind::Elicitation,
                        RequestDetail::Question { question, options },
                        Some(server),
                    )
                }
            }
            "item/permissions/requestApproval" => {
                let permissions = params.get("permissions").cloned().unwrap_or(Value::Null);
                let request = RequestDetail::Command {
                    command: format!("grant permissions {permissions}"),
                    cwd: str_at(params, "/cwd"),
                };
                (PendingKind::Permissions(permissions), request, reason)
            }
            "execCommandApproval" => {
                let command = params
                    .get("command")
                    .and_then(Value::as_array)
                    .map(|argv| {
                        argv.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                let request = RequestDetail::Command {
                    command,
                    cwd: str_at(params, "/cwd"),
                };
                (PendingKind::LegacyDecision, request, reason)
            }
            "applyPatchApproval" => {
                let path = params
                    .get("fileChanges")
                    .and_then(Value::as_object)
                    .map(|changes| changes.keys().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                (
                    PendingKind::LegacyDecision,
                    RequestDetail::FileWrite { path, diff: None },
                    reason,
                )
            }
            _ => {
                // Unanswered, it would block the turn: refuse it.
                let message = format!("Switchyard does not handle {method}");
                self.replies.push(
                    json!({ "id": id, "error": { "code": -32601, "message": message } })
                        .to_string(),
                );
                return vec![];
            }
        };
        let request_id = request_key(id);
        self.pending.insert(
            request_id.clone(),
            Pending {
                id: id.clone(),
                kind,
            },
        );
        vec![Event::PermissionRequest {
            request_id,
            request,
            reason,
        }]
    }

    fn answer(&mut self, request_id: &str, verdict: &Verdict) -> Vec<String> {
        // Already resolved (by the server, or answered before): nothing to send.
        let Some(Pending { id, kind }) = self.pending.remove(request_id) else {
            return vec![];
        };
        let allow = matches!(verdict, Verdict::AllowOnce | Verdict::AllowAlways);
        let result = match kind {
            // "Always allow" is a Switchyard Grant; Codex is only told about this one
            // request, never asked to remember a rule (ADR-0004).
            PendingKind::Decision => {
                json!({ "decision": if allow { "accept" } else { "decline" } })
            }
            PendingKind::LegacyDecision => {
                json!({ "decision": if allow { "approved" } else { "denied" } })
            }
            PendingKind::Questions(ids) => {
                let answers: Map<String, Value> = match verdict {
                    Verdict::Answer { text } => ids
                        .into_iter()
                        .map(|id| (id, json!({ "answers": [text] })))
                        .collect(),
                    _ => Map::new(),
                };
                json!({ "answers": answers })
            }
            PendingKind::McpApproval => {
                json!({ "action": if allow { "accept" } else { "decline" }, "content": null, "_meta": null })
            }
            PendingKind::Elicitation => {
                let accept = allow
                    || matches!(verdict, Verdict::Answer { text } if text.trim().eq_ignore_ascii_case("accept"));
                if accept {
                    json!({ "action": "accept", "content": {}, "_meta": null })
                } else {
                    json!({ "action": "decline", "content": null, "_meta": null })
                }
            }
            PendingKind::Permissions(requested) => {
                let granted = if allow { requested } else { json!({}) };
                json!({ "permissions": granted, "scope": "turn" })
            }
        };
        vec![json!({ "id": id, "result": result }).to_string()]
    }
}

impl Adapter for CodexAdapter {
    fn manifest(&self) -> AdapterManifest {
        manifest()
    }

    fn ingest(&mut self, raw: &str) -> Result<Vec<Event>, AdapterError> {
        let message: Value = serde_json::from_str(raw)
            .map_err(|e| AdapterError(format!("not a JSON-RPC message ({e}): {raw}")))?;
        let method = message.get("method").and_then(Value::as_str);
        let params = message.get("params").unwrap_or(&Value::Null);
        Ok(match (method, message.get("id")) {
            (Some(method), Some(id)) => self.on_server_request(id, method, params),
            (Some(method), None) => self.on_notification(method, params),
            (None, Some(id)) => match id.as_u64() {
                Some(id) => self.on_response(id, &message),
                None => vec![],
            },
            (None, None) => vec![],
        })
    }

    fn command(&mut self, command: &Command) -> Result<Vec<String>, AdapterError> {
        Ok(match command {
            Command::Resume {
                external_session_id,
            } => {
                if self.phase != Phase::New {
                    return Err(AdapterError("resume must be the first command".into()));
                }
                self.resume = Some(external_session_id.clone());
                vec![self.initialize()]
            }
            Command::SendText { text } => match self.phase {
                Phase::New => {
                    self.queued.push(text.clone());
                    vec![self.initialize()]
                }
                Phase::Starting => {
                    self.queued.push(text.clone());
                    vec![]
                }
                Phase::Ready => vec![self.turn_start(vec![text.clone()])],
                Phase::Failed => return Err(AdapterError("Codex could not start a thread".into())),
            },
            Command::AnswerRequest {
                request_id,
                verdict,
            } => self.answer(request_id, verdict),
            Command::Interrupt => {
                self.queued.clear();
                match (self.thread_id.clone(), self.turn_id.clone()) {
                    (Some(thread_id), Some(turn_id)) => {
                        vec![self.request(
                            Call::Other,
                            "turn/interrupt",
                            json!({ "threadId": thread_id, "turnId": turn_id }),
                        )]
                    }
                    _ => vec![],
                }
            }
            Command::SetPermissionMode { permission_mode } => {
                self.launch.permission_mode = *permission_mode;
                if self.phase == Phase::Ready {
                    self.mode_override = Some(*permission_mode);
                }
                vec![]
            }
            Command::SetModel { .. } => {
                return Err(AdapterError("Codex does not switch Models live (switch_model_live is false)".into()))
            }
        })
    }

    fn replies(&mut self) -> Vec<String> {
        std::mem::take(&mut self.replies)
    }
}

/// The thread's config: the launch's own overrides (which define Switchyard's MCP servers
/// for this session), every other server Codex would load on its own (`codex_servers`,
/// from `config/read`) switched off, and the MCP rules for Switchyard's servers. Codex has
/// no strict MCP mode; this is its per-thread equivalent, and nothing is written to its
/// config files. A rule for a server the session does not have is left out: configuring
/// a server that is not defined would leave it half-defined.
pub fn thread_config(launch: &Launch, codex_servers: &BTreeSet<String>) -> Map<String, Value> {
    let ours: BTreeSet<&str> = launch.mcp_servers.iter().map(String::as_str).collect();
    let key = |server: &str, rest: &str| format!("mcp_servers.{}.{rest}", toml_key(server));
    let mut config: BTreeMap<String, Value> = launch.config.clone().into_iter().collect();
    for server in codex_servers.iter().filter(|s| !ours.contains(s.as_str())) {
        config.insert(key(server, "enabled"), json!(false));
    }
    let mut disabled: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let denied = |server: &str, tool: &str| {
        launch
            .mcp_rules
            .iter()
            .any(|r| !r.allow && r.server == server && (r.tool == tool || r.tool == "*"))
    };
    for rule in &launch.mcp_rules {
        let server = rule.server.as_str();
        if !ours.contains(server) {
            continue;
        }
        match (rule.allow, rule.tool.as_str()) {
            (false, "*") => {
                config.insert(key(server, "enabled"), json!(false));
            }
            (false, tool) => disabled.entry(server).or_default().push(json!(tool)),
            (true, _) if denied(server, &rule.tool) => {}
            (true, "*") => {
                config.insert(key(server, "default_tools_approval_mode"), json!("approve"));
            }
            (true, tool) => {
                let field = format!("tools.{}.approval_mode", toml_key(tool));
                config.insert(key(server, &field), json!("approve"));
            }
        }
    }
    for (server, tools) in disabled {
        config.insert(key(server, "disabled_tools"), Value::Array(tools));
    }
    config.into_iter().collect()
}

/// A config.toml key segment: bare when it can be, quoted otherwise.
fn toml_key(segment: &str) -> String {
    let bare = !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        segment.to_string()
    } else {
        Value::String(segment.to_string()).to_string()
    }
}

/// A tool call item as the protocol's tool name and input.
fn tool_call(item: &Value) -> Option<(String, Value)> {
    match item.get("type").and_then(Value::as_str)? {
        "commandExecution" => Some((
            "Shell".into(),
            json!({ "command": str_at(item, "/command").unwrap_or_default(), "cwd": item.get("cwd").cloned().unwrap_or(Value::Null) }),
        )),
        "fileChange" => {
            let paths: Vec<String> = item
                .get("changes")
                .and_then(Value::as_array)
                .map(|changes| changes.iter().filter_map(|c| str_at(c, "/path")).collect())
                .unwrap_or_default();
            Some(("ApplyPatch".into(), json!({ "paths": paths })))
        }
        "mcpToolCall" => Some((
            format!(
                "mcp__{}__{}",
                str_at(item, "/server").unwrap_or_default(),
                str_at(item, "/tool").unwrap_or_default()
            ),
            item.get("arguments").cloned().unwrap_or(Value::Null),
        )),
        "webSearch" => Some((
            "WebSearch".into(),
            json!({ "query": str_at(item, "/query").unwrap_or_default() }),
        )),
        _ => None,
    }
}

/// A JSON-RPC request id as the protocol's request id: numbers in decimal, strings as-is.
fn request_key(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn str_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn error_message(error: &Value) -> String {
    str_at(error, "/message").unwrap_or_else(|| error.to_string())
}

/// `switchyard/0.144.1 (Mac OS ...)` -> `0.144.1`.
fn version_from_user_agent(agent: &str) -> Option<String> {
    let product = agent.split_whitespace().next()?;
    let version = product.split_once('/')?.1;
    (!version.is_empty()).then(|| version.to_string())
}
