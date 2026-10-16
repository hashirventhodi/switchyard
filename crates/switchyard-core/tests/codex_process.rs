//! The real Codex transport: the core spawns `codex app-server` in the worktree and speaks
//! its JSON-RPC through the Codex adapter. Uses a fake app-server shell script (no CLI, no
//! model, no network) that logs what it was started with and sent.

mod harness;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use harness::{started_session, Harness, TempRepo};
use switchyard_core::codex::CodexLauncher;
use switchyard_schema::{AgentKind, Command, Entry, Event, ModelChoice, PermissionMode, SessionStatus, Verdict};

struct FakeCodex {
    _dir: tempfile::TempDir,
    log: PathBuf,
    launcher: Arc<CodexLauncher>,
}

/// The fake script, with its log file passed through the environment of this test process.
fn fake_codex() -> FakeCodex {
    let dir = tempfile::TempDir::new().unwrap();
    let log = dir.path().join("codex.log");
    let script = dir.path().join("codex");
    let body = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-codex.sh"))
        .unwrap()
        .replace("${FAKE_CODEX_LOG:?}", &log.to_string_lossy());
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    FakeCodex { _dir: dir, log, launcher: Arc::new(CodexLauncher { codex: script, record_dir: None }) }
}

fn start(worktree: &Path, prompt: &str) -> Command {
    Command::StartSession {
        worktree_path: worktree.to_string_lossy().into_owned(),
        agent: AgentKind::Codex,
        permission_mode: PermissionMode::AskFirst,
        model: Default::default(),
        prompt: prompt.into(),
    }
}

fn has_status(events: &[Event], wanted: SessionStatus) -> bool {
    events.iter().any(|e| matches!(e, Event::SessionStatusChanged { status, .. } if *status == wanted))
}

fn agent_texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { entry: Entry::Agent { text }, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn sent(log: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .filter_map(|l| l.strip_prefix("in="))
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn codex_app_server_runs_in_the_worktree_and_its_turn_reaches_the_transcript() {
    let fake = fake_codex();
    let repo = TempRepo::new("acme");
    std::fs::write(repo.path.join(".mcp.json"), r#"{"mcpServers":{"docs":{"command":"npx","args":["-y","docs-mcp"]}}}"#)
        .unwrap();
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch_with(fake.launcher.clone());
    let repository_id = harness::added_repository(&core.add_repository(&repo.path)).id;
    core.dispatch(Command::EnableMcpServer { repository_id, name: "docs".into() });

    core.dispatch(start(&worktree, "hello"));
    let events = core.wait_for(|seen| has_status(seen, SessionStatus::Waiting));

    assert_eq!(agent_texts(&events), vec!["Hello from Codex.".to_string()]);
    let log = std::fs::read_to_string(&fake.log).unwrap();
    assert!(log.contains("args=app-server\n"), "{log}");
    assert!(log.contains(&format!("cwd={}\n", worktree.canonicalize().unwrap().display())), "{log}");
    let sent = sent(&fake.log);
    let methods: Vec<&str> = sent.iter().filter_map(|m| m["method"].as_str()).collect();
    assert_eq!(methods, vec!["initialize", "initialized", "config/read", "thread/start", "turn/start", "model/list"]);
    let thread = &sent[3]["params"];
    assert_eq!(thread["cwd"], worktree.to_string_lossy().as_ref());
    assert_eq!(thread["approvalPolicy"], "untrusted");
    assert_eq!(thread["sandbox"], "workspace-write");
    // Codex's own MCP server is off for this thread; the repository's is defined.
    assert_eq!(thread["config"]["mcp_servers.notion.enabled"], false);
    assert_eq!(thread["config"]["mcp_servers.docs.command"], "npx");
    assert_eq!(sent[4]["params"]["threadId"], "thread-fake");
    assert_eq!(sent[4]["params"]["input"][0]["text"], "hello");
}

#[test]
fn codex_and_the_mcp_servers_it_starts_get_granted_secrets_and_none_of_the_others() {
    // Switchyard itself was started with an ungranted secret in its environment.
    std::env::set_var("SY26_CX_STRIPE_KEY", "sk-must-not-leak");
    let fake = fake_codex();
    let repo = TempRepo::new("acme");
    std::fs::create_dir_all(repo.path.join(".switchyard")).unwrap();
    std::fs::write(repo.path.join(".switchyard/policy.yml"), "secrets: [SY26_CX_STRIPE_KEY]\n").unwrap();
    std::fs::write(
        repo.path.join(".mcp.json"),
        r#"{"mcpServers":{"npm":{"command":"npx","args":["-y","npm-mcp"],"env":{"SY26_CX_NPM_TOKEN":"${SY26_CX_NPM_TOKEN}"}}}}"#,
    )
    .unwrap();
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch_with(fake.launcher.clone());
    let repository_id = harness::added_repository(&core.add_repository(&repo.path)).id;
    core.secrets.in_keychain("SY26_CX_NPM_TOKEN", "npm-123");
    core.dispatch(Command::GrantSecret { repository_id: repository_id.clone(), name: "SY26_CX_NPM_TOKEN".into() });
    core.dispatch(Command::EnableMcpServer { repository_id, name: "npm".into() });

    core.dispatch(start(&worktree, "hello"));
    core.wait_for(|seen| has_status(seen, SessionStatus::Waiting));

    let log = std::fs::read_to_string(&fake.log).unwrap();
    assert!(log.contains("env=SY26_CX_NPM_TOKEN=npm-123\n"), "{log}");
    assert!(!log.contains("SY26_CX_STRIPE_KEY"), "{log}");
    assert!(!log.contains("sk-must-not-leak"), "{log}");
    // The MCP server inherits the granted secret from Codex by name; no value is in the config.
    let thread = &sent(&fake.log)[3]["params"];
    assert_eq!(thread["config"]["mcp_servers.npm.env_vars"], serde_json::json!(["SY26_CX_NPM_TOKEN"]));
    assert!(thread["config"].get("mcp_servers.npm.env.SY26_CX_NPM_TOKEN").is_none(), "{thread:#}");
    assert!(!thread.to_string().contains("npm-123"), "{thread:#}");
}

#[test]
fn a_codex_approval_is_answered_with_the_same_json_rpc_id() {
    let fake = fake_codex();
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch_with(fake.launcher.clone());
    core.add_repository(&repo.path);

    core.dispatch(start(&worktree, "ask first"));
    let events = core.wait_for(|seen| has_status(seen, SessionStatus::Waiting));
    let id = started_session(&events);
    core.dispatch(Command::AnswerRequest { session_id: id, request_id: "0".into(), verdict: Verdict::AllowOnce });
    let events = core.wait_for(|seen| has_status(seen, SessionStatus::Waiting));

    assert_eq!(agent_texts(&events), vec!["Tests ran.".to_string()]);
    let answers: Vec<serde_json::Value> = sent(&fake.log).into_iter().filter(|m| m.get("result").is_some()).collect();
    assert_eq!(answers, vec![serde_json::json!({ "id": 0, "result": { "decision": "accept" } })]);
}

fn start_on(worktree: &Path, model: &str, prompt: &str) -> Command {
    Command::StartSession {
        worktree_path: worktree.to_string_lossy().into_owned(),
        agent: AgentKind::Codex,
        permission_mode: PermissionMode::AskFirst,
        model: ModelChoice::Model { id: model.into() },
        prompt: prompt.into(),
    }
}

fn thread_calls(log: &Path) -> Vec<serde_json::Value> {
    sent(log).into_iter().filter(|m| matches!(m["method"].as_str(), Some("thread/start" | "thread/resume"))).collect()
}

fn switch_state(events: &[Event]) -> Option<(Option<String>, Option<String>)> {
    events.iter().rev().find_map(|e| match e {
        Event::SessionModelChanged { pending_model, switched_model, .. } => {
            Some((pending_model.clone(), switched_model.clone()))
        }
        _ => None,
    })
}

#[test]
fn a_codex_session_starts_on_its_chosen_model_and_resumes_on_its_current_one() {
    let fake = fake_codex();
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch_with(fake.launcher.clone());
    core.add_repository(&repo.path);

    core.dispatch(start_on(&worktree, "gpt-5.6-sol", "hello"));
    let events = core.wait_for(|seen| has_status(seen, SessionStatus::Waiting));
    let id = started_session(&events);
    assert_eq!(thread_calls(&fake.log)[0]["method"], "thread/start");
    assert_eq!(thread_calls(&fake.log)[0]["params"]["model"], "gpt-5.6-sol");

    // Codex cannot switch live: the switch waits for the resume, which runs on it.
    core.dispatch(Command::SetModel { session_id: id.clone(), model: "gpt-5.6-luna".into() });
    core.dispatch(Command::StopSession { session_id: id.clone() });
    core.dispatch(Command::ResumeSession { session_id: id });
    let events = core.wait_for(|seen| {
        switch_state(seen) == Some((None, Some("gpt-5.6-luna".into()))) && has_status(seen, SessionStatus::Waiting)
    });

    let resumed = &thread_calls(&fake.log)[1];
    assert_eq!(resumed["method"], "thread/resume");
    assert_eq!(resumed["params"]["threadId"], "thread-fake");
    assert_eq!(resumed["params"]["model"], "gpt-5.6-luna");
    assert!(sent(&fake.log).iter().all(|m| m["method"] != "set_model"));
    assert!(agent_texts(&events).contains(&"Hello from Codex.".to_string()), "{events:#?}");
}
