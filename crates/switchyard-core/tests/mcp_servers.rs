//! MCP servers per spawn through seam 1: the repository's servers (its `.mcp.json`) reach
//! the launcher, and only those; nothing is written into any CLI config file (ADR-0004);
//! each server's connection state comes from the session init (and later status reports)
//! and lands on the session's facts for Context.
//!
//! `.mcp.json` is committed, so it declares servers but enables none (ADR-0006): a server
//! is passed only once the user enabled it, for exactly the definition they saw.
mod harness;

use std::collections::BTreeMap;
use std::path::PathBuf;

use harness::{added_repository, started_session, Harness, TempRepo};
use switchyard_core::mcp::{McpServer, McpTransport};
use adapter_protocol::{CliInfo, Event as AgentEvent, McpServerState, McpServerStatus, PermissionMode as AgentMode};
use switchyard_schema::{Command, DeclaredMcpServer, Event, McpServerFact, McpState, PermissionMode};

struct Setup {
    core: Harness,
    repo: TempRepo,
    repo_id: String,
    worktree: PathBuf,
}

fn setup(mcp_json: Option<&str>) -> Setup {
    let repo = TempRepo::new("acme");
    if let Some(text) = mcp_json {
        std::fs::write(repo.path.join(".mcp.json"), text).unwrap();
        harness::git(&repo.path, &["add", ".mcp.json"]);
        harness::git(&repo.path, &["commit", "-q", "-m", "mcp"]);
    }
    let worktree = repo.add_worktree("feat/mcp");
    let mut core = Harness::launch();
    let repo_id = added_repository(&core.add_repository(&repo.path)).id;
    Setup { core, repo, repo_id, worktree }
}

impl Setup {
    fn enable(&mut self, name: &str) -> Vec<Event> {
        self.core.send(Command::EnableMcpServer { repository_id: self.repo_id.clone(), name: name.into() })
    }

    fn declared(&mut self) -> Vec<DeclaredMcpServer> {
        let events = self.core.send(Command::ReadMcpServers { repository_id: self.repo_id.clone() });
        declared_in(&events)
    }
}

fn declared_in(events: &[Event]) -> Vec<DeclaredMcpServer> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::McpServersChanged { servers, .. } => Some(servers.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no mcpServersChanged in {events:#?}"))
}

/// Both servers enabled, as most tests here want them.
fn setup_enabled(mcp_json: &str) -> Setup {
    let mut s = setup(Some(mcp_json));
    s.enable("docs");
    s.enable("github");
    s
}

const TWO_SERVERS: &str = r#"{
  "mcpServers": {
    "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": { "GITHUB_TOKEN": "${GITHUB_TOKEN}" } },
    "docs": { "type": "http", "url": "https://docs.example.com/mcp", "headers": { "X-Team": "acme" } }
  }
}"#;

fn github() -> McpServer {
    McpServer {
        name: "github".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            env: BTreeMap::from([("GITHUB_TOKEN".into(), "${GITHUB_TOKEN}".into())]),
        },
    }
}

fn docs() -> McpServer {
    McpServer {
        name: "docs".into(),
        transport: McpTransport::Http {
            url: "https://docs.example.com/mcp".into(),
            headers: BTreeMap::from([("X-Team".into(), "acme".into())]),
        },
    }
}

#[test]
fn the_spawn_carries_the_repositorys_servers_and_only_them() {
    let mut s = setup_enabled(TWO_SERVERS);
    let path = s.worktree.clone();
    started_session(&s.core.start_session_in(&path, PermissionMode::AskFirst, "Ship it."));
    assert_eq!(s.core.agents.launches()[0].mcp_servers, vec![docs(), github()]);
}

fn init(servers: Vec<McpServerStatus>) -> AgentEvent {
    AgentEvent::SessionInit {
        external_session_id: "ext-1".into(),
        permission_mode: AgentMode::AskFirst,
        cli_permission_mode: "default".into(),
        cli: CliInfo { name: "claude-code".into(), version: "2.1.300".into() },
        model: None,
        mcp_servers: servers,
    }
}

fn status(name: &str, state: McpServerState, error: Option<&str>) -> McpServerStatus {
    McpServerStatus { name: name.into(), state, error: error.map(Into::into) }
}

fn fact(name: &str, transport: &str, state: McpState, error: Option<&str>) -> McpServerFact {
    McpServerFact { name: name.into(), transport: transport.into(), state, error: error.map(Into::into) }
}

fn last_mcp_facts(events: &[Event]) -> Vec<McpServerFact> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionFactsChanged { facts, .. } => Some(facts.mcp_servers.clone()),
            _ => None,
        })
        .expect("facts changed")
}

#[test]
fn context_shows_each_servers_state_from_the_init_event() {
    let mut s = setup_enabled(TWO_SERVERS);
    let path = s.worktree.clone();
    started_session(&s.core.start_session_in(&path, PermissionMode::AskFirst, "Ship it."));
    let events = s.core.agent_says(
        0,
        init(vec![
            status("docs", McpServerState::Connected, None),
            status("github", McpServerState::Failed, Some("spawn npx ENOENT")),
        ]),
    );
    assert_eq!(
        last_mcp_facts(&events),
        vec![
            fact("docs", "http · docs.example.com", McpState::Connected, None),
            fact("github", "stdio · npx", McpState::Failed, Some("spawn npx ENOENT")),
        ]
    );
}

#[test]
fn a_server_the_cli_did_not_report_shows_as_not_connected() {
    let mut s = setup_enabled(TWO_SERVERS);
    let path = s.worktree.clone();
    started_session(&s.core.start_session_in(&path, PermissionMode::AskFirst, "Ship it."));
    let events = s.core.agent_says(0, init(vec![status("docs", McpServerState::Connected, None)]));
    assert_eq!(
        last_mcp_facts(&events)[1],
        fact("github", "stdio · npx", McpState::Failed, Some("Claude Code did not load it"))
    );
}

#[test]
fn a_server_pending_at_init_settles_from_a_later_status_report() {
    let mut s = setup_enabled(TWO_SERVERS);
    let path = s.worktree.clone();
    started_session(&s.core.start_session_in(&path, PermissionMode::AskFirst, "Ship it."));
    s.core.agent_says(
        0,
        init(vec![status("docs", McpServerState::Pending, None), status("github", McpServerState::Connected, None)]),
    );
    let events = s.core.agent_says(
        0,
        AgentEvent::McpStatus { servers: vec![status("docs", McpServerState::NeedsAuth, None)] },
    );
    assert_eq!(
        last_mcp_facts(&events),
        vec![
            fact("docs", "http · docs.example.com", McpState::NeedsAuth, None),
            fact("github", "stdio · npx", McpState::Connected, None),
        ]
    );
}

fn refusals(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CommandRefused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_repository_without_mcp_json_spawns_with_no_servers() {
    let mut s = setup(None);
    let path = s.worktree.clone();
    started_session(&s.core.start_session_in(&path, PermissionMode::AskFirst, "Ship it."));
    assert_eq!(s.core.agents.launches()[0].mcp_servers, vec![]);
}

#[test]
fn a_broken_mcp_json_refuses_the_start_naming_the_file() {
    let mut s = setup(Some(r#"{ "mcpServers": { "github": { "args": [] } } }"#));
    let path = s.worktree.clone();
    let events = s.core.start_session_in(&path, PermissionMode::AskFirst, "Ship it.");
    assert_eq!(
        refusals(&events),
        vec!["Could not start the session: .mcp.json is not valid: server github has no command".to_string()]
    );
    assert!(s.core.agents.launches().is_empty());
}

#[test]
fn claude_code_gets_the_servers_as_its_mcp_config() {
    assert_eq!(
        switchyard_core::mcp::claude_servers(&[docs(), github()]),
        serde_json::json!({
            "docs": { "type": "http", "url": "https://docs.example.com/mcp", "headers": { "X-Team": "acme" } },
            "github": {
                "type": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"],
                "env": { "GITHUB_TOKEN": "${GITHUB_TOKEN}" }
            }
        })
    );
}

#[test]
fn codex_gets_the_servers_as_config_overrides() {
    let pairs: Vec<String> = switchyard_core::mcp::codex_config(&[docs(), github()])
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    assert_eq!(
        pairs,
        vec![
            r#"mcp_servers.docs.url="https://docs.example.com/mcp""#,
            r#"mcp_servers.docs.http_headers.X-Team="acme""#,
            r#"mcp_servers.github.command="npx""#,
            r#"mcp_servers.github.args=["-y","@modelcontextprotocol/server-github"]"#,
            r#"mcp_servers.github.env.GITHUB_TOKEN="${GITHUB_TOKEN}""#,
        ]
    );
}

#[test]
fn codex_mcp_servers_inherit_the_granted_secrets_by_name_and_no_values() {
    use switchyard_core::secrets::SpawnEnv;
    let env = SpawnEnv {
        set: BTreeMap::from([("NPM_TOKEN".into(), "npm-123".into())]),
        remove: ["GITHUB_TOKEN".to_string()].into(),
    };
    let pairs: Vec<String> = switchyard_core::mcp::codex_config_for_spawn(&[docs(), github()], &env)
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    assert_eq!(
        pairs,
        vec![
            r#"mcp_servers.docs.url="https://docs.example.com/mcp""#,
            r#"mcp_servers.docs.http_headers.X-Team="acme""#,
            r#"mcp_servers.github.command="npx""#,
            r#"mcp_servers.github.args=["-y","@modelcontextprotocol/server-github"]"#,
            r#"mcp_servers.github.env_vars=["NPM_TOKEN"]"#,
        ]
    );
}

#[test]
fn context_names_a_servers_transport_without_its_arguments() {
    assert_eq!(github().summary(), "stdio · npx");
    let with_token = McpServer {
        name: "x".into(),
        transport: McpTransport::Http {
            url: "https://user:tok@mcp.example.com/v1?key=s".into(),
            headers: BTreeMap::new(),
        },
    };
    assert_eq!(with_token.summary(), "http · mcp.example.com");
}

#[test]
fn a_committed_server_is_declared_but_not_enabled_and_never_passed_until_enabled() {
    let mut s = setup(Some(TWO_SERVERS));
    let declared = s.declared();
    let summary: Vec<(&str, &str, bool, bool)> =
        declared.iter().map(|d| (d.name.as_str(), d.transport.as_str(), d.enabled, d.changed)).collect();
    assert_eq!(summary, vec![("docs", "http · docs.example.com", false, false), ("github", "stdio · npx", false, false)]);

    let path = s.worktree.clone();
    started_session(&s.core.start_session_in(&path, PermissionMode::AskFirst, "Ship it."));
    assert_eq!(s.core.agents.launches()[0].mcp_servers, vec![], "nothing declared is passed");

    let events = s.enable("github");
    assert!(declared_in(&events).iter().any(|d| d.name == "github" && d.enabled));
    s.core.send(Command::StopSession { session_id: "session-1".into() });
    let other = s.repo.add_worktree("feat/two");
    started_session(&s.core.start_session_in(&other, PermissionMode::AskFirst, "Again."));
    assert_eq!(s.core.agents.launches()[1].mcp_servers, vec![github()]);
}

#[test]
fn a_changed_definition_asks_again_and_is_not_passed_meanwhile() {
    let mut s = setup_enabled(TWO_SERVERS);
    let changed = TWO_SERVERS.replace("@modelcontextprotocol/server-github", "@evil/server-github");
    std::fs::write(s.repo.path.join(".mcp.json"), changed).unwrap();

    let github_now = s.declared().into_iter().find(|d| d.name == "github").unwrap();
    assert!(!github_now.enabled && github_now.changed, "{github_now:?}");
    let path = s.worktree.clone();
    started_session(&s.core.start_session_in(&path, PermissionMode::AskFirst, "Ship it."));
    assert_eq!(s.core.agents.launches()[0].mcp_servers, vec![docs()]);
}

#[test]
fn disabling_a_server_stops_passing_it_and_unknown_names_are_refused() {
    let mut s = setup_enabled(TWO_SERVERS);
    let events = s.core.send(Command::DisableMcpServer { repository_id: s.repo_id.clone(), name: "docs".into() });
    assert!(declared_in(&events).iter().any(|d| d.name == "docs" && !d.enabled));
    let path = s.worktree.clone();
    started_session(&s.core.start_session_in(&path, PermissionMode::AskFirst, "Ship it."));
    assert_eq!(s.core.agents.launches()[0].mcp_servers, vec![github()]);

    let events = s.enable("nope");
    assert_eq!(refusals(&events), vec![".mcp.json declares no server nope".to_string()]);
}
