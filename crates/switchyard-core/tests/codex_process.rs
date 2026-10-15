//! The real Codex transport: the core spawns `codex app-server` in the worktree and speaks
//! its JSON-RPC through the Codex adapter. Uses a fake app-server shell script (no CLI, no
//! model, no network) that logs what it was started with and sent.

mod harness;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use harness::{started_session, Harness, TempRepo};
use switchyard_core::codex::CodexLauncher;
use switchyard_schema::{AgentKind, Command, Entry, Event, PermissionMode, SessionStatus, Verdict};

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
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch_with(fake.launcher.clone());
    core.add_repository(&repo.path);

    core.dispatch(start(&worktree, "hello"));
    let events = core.wait_for(|seen| has_status(seen, SessionStatus::Waiting));

    assert_eq!(agent_texts(&events), vec!["Hello from Codex.".to_string()]);
    let log = std::fs::read_to_string(&fake.log).unwrap();
    assert!(log.contains("args=app-server\n"), "{log}");
    assert!(log.contains(&format!("cwd={}\n", worktree.canonicalize().unwrap().display())), "{log}");
    let sent = sent(&fake.log);
    let methods: Vec<&str> = sent.iter().filter_map(|m| m["method"].as_str()).collect();
    assert_eq!(methods, vec!["initialize", "initialized", "thread/start", "turn/start"]);
    let thread = &sent[2]["params"];
    assert_eq!(thread["cwd"], worktree.to_string_lossy().as_ref());
    assert_eq!(thread["approvalPolicy"], "untrusted");
    assert_eq!(thread["sandbox"], "workspace-write");
    assert_eq!(sent[3]["params"]["threadId"], "thread-fake");
    assert_eq!(sent[3]["params"]["input"][0]["text"], "hello");
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
