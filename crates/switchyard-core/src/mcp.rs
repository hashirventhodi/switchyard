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
//! (`${VAR}` included): expanding them, and choosing which secrets an MCP server process
//! may inherit, is the secrets grant's job.
//!
//! [`LaunchSpec::mcp_servers`]: crate::agents::LaunchSpec::mcp_servers

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Map, Value};

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

/// A TOML key segment: bare when it can be, quoted otherwise.
fn toml_key(key: &str) -> String {
    let bare = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        key.to_string()
    } else {
        Value::String(key.to_string()).to_string()
    }
}
