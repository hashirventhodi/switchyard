//! The Repository's MCP servers: `.mcp.json` in its primary worktree, the shared
//! `{"mcpServers": {...}}` file format, read only. They reach each CLI per spawn through
//! that CLI's own per-invocation flags ([`LaunchSpec::mcp_servers`]); nothing is ever
//! written into a CLI's config files (ADR-0004).
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"],
//!                 "env": { "GITHUB_TOKEN": "${GITHUB_TOKEN}" } },
//!     "docs":   { "type": "http", "url": "https://docs.example.com/mcp" }
//!   }
//! }
//! ```
//!
//! The model here is CLI-agnostic; each launcher maps it onto its CLI ([`claude_servers`]
//! for the Claude Code sidecar, [`codex_config`] for Codex). Values are passed verbatim
//! (`${VAR}` included) and never expanded by Switchyard: an MCP server process sees only the
//! secrets granted to the repository, because the agent process it inherits from was spawned
//! with only those (`crate::secrets`; [`codex_config_for_spawn`] for Codex).
//!
//! `.mcp.json` is committed, so it only declares servers (ADR-0006): a server reaches an
//! agent once the user has enabled it ([`enabled`]), an Acknowledgement pinned to a hash
//! of its definition, so any change to the definition asks again.
//!
//! [`LaunchSpec::mcp_servers`]: crate::agents::LaunchSpec::mcp_servers

use std::collections::BTreeMap;
use std::path::Path;

use adapter_protocol::{McpServerState, McpServerStatus};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use switchyard_schema::{McpServerFact, McpState};

pub const MCP_FILE: &str = ".mcp.json";

/// One MCP server a Session's CLI is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransport {
    /// A process the CLI starts and talks to over stdio.
    Stdio { command: String, args: Vec<String>, env: BTreeMap<String, String> },
    /// Streamable HTTP.
    Http { url: String, headers: BTreeMap<String, String> },
    /// Server-sent events (the older HTTP transport).
    Sse { url: String, headers: BTreeMap<String, String> },
}

impl McpServer {
    /// How Context names the server's transport, without its arguments, environment or
    /// headers (they may carry tokens).
    pub fn summary(&self) -> String {
        match &self.transport {
            McpTransport::Stdio { command, .. } => format!("stdio · {command}"),
            McpTransport::Http { url, .. } => format!("http · {}", host(url)),
            McpTransport::Sse { url, .. } => format!("sse · {}", host(url)),
        }
    }
}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.rsplit_once('@').map_or(host, |(_, host)| host)
}

#[derive(Deserialize)]
struct File {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: Option<String>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

/// The servers of the repository whose primary worktree is `primary`, by name. A missing
/// file is no servers; a file that cannot be read or parsed is an error naming it.
pub fn load(primary: &Path) -> Result<Vec<McpServer>, String> {
    let path = primary.join(MCP_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {MCP_FILE}: {e}")),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let invalid = |e: String| format!("{MCP_FILE} is not valid: {e}");
    let file: File = serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?;
    file.mcp_servers
        .into_iter()
        .map(|(name, entry)| {
            let transport = match (entry.kind.as_deref(), entry.command, entry.url) {
                (None | Some("stdio"), Some(command), _) => {
                    McpTransport::Stdio { command, args: entry.args, env: entry.env }
                }
                (Some("http"), _, Some(url)) => McpTransport::Http { url, headers: entry.headers },
                (Some("sse"), _, Some(url)) => McpTransport::Sse { url, headers: entry.headers },
                (None | Some("stdio"), None, _) => return Err(invalid(format!("server {name} has no command"))),
                (Some("http" | "sse"), _, None) => return Err(invalid(format!("server {name} has no url"))),
                (Some(other), _, _) => return Err(invalid(format!("server {name} has unknown type {other}"))),
            };
            Ok(McpServer { name, transport })
        })
        .collect()
}

/// The environment variables the servers reference as `${NAME}` (or `${NAME:-default}`)
/// in their command, arguments, environment, URL or headers: secrets they expect.
pub fn secret_refs(servers: &[McpServer]) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    let mut scan = |text: &str| {
        let mut rest = text;
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            let name = rest[..end].split(":-").next().unwrap_or_default();
            if crate::secrets::valid_name(name) {
                names.insert(name.to_string());
            }
            rest = &rest[end + 1..];
        }
    };
    for server in servers {
        match &server.transport {
            McpTransport::Stdio { command, args, env } => {
                scan(command);
                args.iter().for_each(|a| scan(a));
                env.values().for_each(|v| scan(v));
            }
            McpTransport::Http { url, headers } | McpTransport::Sse { url, headers } => {
                scan(url);
                headers.values().for_each(|v| scan(v));
            }
        }
    }
    names
}

/// The servers as the Claude Agent SDK's `mcpServers` option (the CLI's `--mcp-config`
/// JSON), by name.
pub fn claude_servers(servers: &[McpServer]) -> Value {
    let map: Map<String, Value> = servers
        .iter()
        .map(|server| {
            let config = match &server.transport {
                McpTransport::Stdio { command, args, env } => {
                    json!({ "type": "stdio", "command": command, "args": args, "env": env })
                }
                McpTransport::Http { url, headers } => json!({ "type": "http", "url": url, "headers": headers }),
                McpTransport::Sse { url, headers } => json!({ "type": "sse", "url": url, "headers": headers }),
            };
            (server.name.clone(), config)
        })
        .collect();
    Value::Object(map)
}

/// The servers as Codex config overrides: dotted `config.toml` keys and their values, in
/// order. Each pair is one `codex -c '<key>=<value>'` (the value written as JSON, which is
/// valid TOML for strings and string arrays) or one entry of an app-server
/// `thread/start` `config` map. Codex has no strict mode: user-level servers from
/// `~/.codex/config.toml` are not excluded by these.
pub fn codex_config(servers: &[McpServer]) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for server in servers {
        let key = |field: &str| format!("mcp_servers.{}.{field}", toml_key(&server.name));
        match &server.transport {
            McpTransport::Stdio { command, args, env } => {
                out.push((key("command"), json!(command)));
                out.push((key("args"), json!(args)));
                for (name, value) in env {
                    out.push((key(&format!("env.{}", toml_key(name))), json!(value)));
                }
            }
            McpTransport::Http { url, headers } | McpTransport::Sse { url, headers } => {
                out.push((key("url"), json!(url)));
                for (name, value) in headers {
                    out.push((key(&format!("http_headers.{}", toml_key(name))), json!(value)));
                }
            }
        }
    }
    out
}

/// [`codex_config`] for one spawn with its secrets ([`LaunchSpec::env`]). Codex starts a
/// stdio MCP server with a minimal environment and does not expand `${VAR}`, so each stdio
/// server is told to inherit the granted secrets by name (`env_vars`), and a `NAME:
/// "${NAME}"` entry (a plain pass-through) is left to that rather than sent as a literal.
/// Names only: no secret value is ever in these pairs, so they are safe as `-c` arguments.
///
/// [`LaunchSpec::env`]: crate::agents::LaunchSpec::env
pub fn codex_config_for_spawn(servers: &[McpServer], env: &crate::secrets::SpawnEnv) -> Vec<(String, Value)> {
    let granted: Vec<&String> = env.set.keys().collect();
    let passed_through = |(name, value): (&String, &String)| *value == format!("${{{name}}}");
    let servers: Vec<McpServer> = servers
        .iter()
        .map(|server| match &server.transport {
            McpTransport::Stdio { command, args, env } => McpServer {
                name: server.name.clone(),
                transport: McpTransport::Stdio {
                    command: command.clone(),
                    args: args.clone(),
                    env: env.iter().filter(|e| !passed_through(*e)).map(|(k, v)| (k.clone(), v.clone())).collect(),
                },
            },
            _ => server.clone(),
        })
        .collect();
    let mut out = Vec::new();
    for server in &servers {
        out.extend(codex_config(std::slice::from_ref(server)));
        if matches!(server.transport, McpTransport::Stdio { .. }) && !granted.is_empty() {
            out.push((format!("mcp_servers.{}.env_vars", toml_key(&server.name)), json!(granted)));
        }
    }
    out
}

/// The servers a Session was given, as Context lists them before the CLI reports on them.
pub(crate) fn facts(servers: &[McpServer]) -> Vec<McpServerFact> {
    servers
        .iter()
        .map(|server| McpServerFact {
            name: server.name.clone(),
            transport: server.summary(),
            state: McpState::Pending,
            error: None,
        })
        .collect()
}

/// The CLI's report at session init: every server it was given and their states. One it
/// does not mention never loaded, so it is not connected.
pub(crate) fn on_init(facts: &mut Vec<McpServerFact>, reported: Vec<McpServerStatus>, cli_name: &str) {
    for fact in facts.iter_mut() {
        fact.state = McpState::Failed;
        fact.error = Some(format!("{cli_name} did not load it"));
    }
    on_status(facts, reported);
}

/// A report on some servers; the others keep their state. A server Switchyard did not
/// pass (which strict mode should rule out) is listed as reported.
pub(crate) fn on_status(facts: &mut Vec<McpServerFact>, reported: Vec<McpServerStatus>) {
    for status in reported {
        let state = match status.state {
            McpServerState::Connected => McpState::Connected,
            McpServerState::Pending => McpState::Pending,
            McpServerState::Failed => McpState::Failed,
            McpServerState::NeedsAuth => McpState::NeedsAuth,
            McpServerState::Disabled => McpState::Disabled,
        };
        match facts.iter_mut().find(|f| f.name == status.name) {
            Some(fact) => {
                fact.state = state;
                fact.error = status.error;
            }
            None => facts.push(McpServerFact {
                name: status.name,
                transport: "not from this repository".into(),
                state,
                error: status.error,
            }),
        }
    }
}

/// A TOML key segment: bare when it can be, quoted otherwise.
fn toml_key(key: &str) -> String {
    let bare = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        key.to_string()
    } else {
        Value::String(key.to_string()).to_string()
    }
}

/// A server's definition as it is pinned when enabled: everything that decides what runs
/// or where it connects (command, args, env, url, headers).
fn definition_hash(server: &McpServer) -> String {
    let definition = match &server.transport {
        McpTransport::Stdio { command, args, env } => json!({ "type": "stdio", "command": command, "args": args, "env": env }),
        McpTransport::Http { url, headers } => json!({ "type": "http", "url": url, "headers": headers }),
        McpTransport::Sse { url, headers } => json!({ "type": "sse", "url": url, "headers": headers }),
    };
    crate::acknowledgements::hash(&format!("{}\n{definition}", server.name))
}

fn subject(name: &str) -> String {
    format!("mcp:{name}")
}

/// The repository's servers the user has enabled for their current definition: the only
/// ones passed to agents. A broken `.mcp.json` is an error naming it.
pub(crate) fn enabled(inner: &crate::Inner, repository_id: &str, primary: &Path) -> Result<Vec<McpServer>, String> {
    let db = inner.store.db();
    Ok(load(primary)?
        .into_iter()
        .filter(|s| crate::acknowledgements::holds(db, repository_id, &subject(&s.name), &definition_hash(s)))
        .collect())
}

pub(crate) fn announce(inner: &crate::Inner, repo: &crate::store::StoredRepository) {
    let db = inner.store.db();
    let (servers, error) = match load(Path::new(&repo.path)) {
        Ok(servers) => (servers, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    let servers = servers
        .iter()
        .map(|s| {
            let acknowledged = crate::acknowledgements::get(db, &repo.id, &subject(&s.name));
            let enabled = acknowledged.as_deref() == Some(definition_hash(s).as_str());
            switchyard_schema::DeclaredMcpServer {
                name: s.name.clone(),
                transport: s.summary(),
                enabled,
                changed: acknowledged.is_some() && !enabled,
            }
        })
        .collect();
    inner.emit(switchyard_schema::Event::McpServersChanged { repository_id: repo.id.clone(), servers, error });
}

/// [`Command::ReadMcpServers`](switchyard_schema::Command::ReadMcpServers).
pub(crate) fn read(inner: &crate::Inner, repository_id: &str) {
    match crate::worktrees::repository(inner, repository_id) {
        Some(repo) => announce(inner, &repo),
        None => inner.refuse(switchyard_schema::command_name::READ_MCP_SERVERS, crate::no_repository(repository_id)),
    }
}

/// [`Command::EnableMcpServer`] / [`Command::DisableMcpServer`]: takes effect at the next
/// spawn.
///
/// [`Command::EnableMcpServer`]: switchyard_schema::Command::EnableMcpServer
/// [`Command::DisableMcpServer`]: switchyard_schema::Command::DisableMcpServer
pub(crate) fn set_enabled(inner: &crate::Inner, repository_id: &str, name: &str, on: bool) {
    use switchyard_schema::command_name::{DISABLE_MCP_SERVER, ENABLE_MCP_SERVER};
    let command = if on { ENABLE_MCP_SERVER } else { DISABLE_MCP_SERVER };
    let Some(repo) = crate::worktrees::repository(inner, repository_id) else {
        return inner.refuse(command, crate::no_repository(repository_id));
    };
    let db = inner.store.db();
    let saved = if on {
        let servers = match load(Path::new(&repo.path)) {
            Ok(servers) => servers,
            Err(reason) => return inner.refuse(command, reason),
        };
        let Some(server) = servers.iter().find(|s| s.name == name) else {
            return inner.refuse(command, format!("{MCP_FILE} declares no server {name}"));
        };
        crate::acknowledgements::set(db, &repo.id, &subject(name), &definition_hash(server))
    } else {
        crate::acknowledgements::remove(db, &repo.id, &subject(name))
    };
    if let Err(reason) = saved {
        return inner.refuse(command, format!("Could not change {name}: {reason}"));
    }
    announce(inner, &repo);
}
