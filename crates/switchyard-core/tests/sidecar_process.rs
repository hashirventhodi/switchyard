//! The real agent transport: the core spawns a sidecar process in the worktree and speaks
//! the adapter protocol with it over stdio. Uses a fake sidecar script under Node (no SDK,
//! no CLI, no model).

mod harness;

use std::path::PathBuf;
use std::sync::Arc;

use harness::{started_session, Harness, TempRepo};
use switchyard_core::sidecar::SidecarLauncher;
use switchyard_schema::{Command, Entry, Event, SessionStatus};

fn fake_sidecar() -> Arc<SidecarLauncher> {
    Arc::new(SidecarLauncher {
        node: PathBuf::from("node"),
        script: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-sidecar.mjs"),
        claude: None,
    })
}

fn start(worktree: &std::path::Path) -> Command {
    Command::StartSession {
        worktree_path: worktree.to_string_lossy().into_owned(),
        agent: switchyard_schema::AgentKind::ClaudeCode,
        permission_mode: switchyard_schema::PermissionMode::AskFirst,
        prompt: "hello".into(),
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

#[test]
fn the_sidecar_runs_in_the_worktree_and_streams_protocol_events_back() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch_with(fake_sidecar());
    core.add_repository(&repo.path);

    core.dispatch(start(&worktree));
    let events = core.wait_for(|seen| has_status(seen, SessionStatus::Waiting));

    assert_eq!(
        agent_texts(&events),
        vec![format!("cwd={}", worktree.canonicalize().unwrap().display()), "you said: hello".to_string()]
    );
    let facts = events.iter().find_map(|e| match e {
        Event::SessionFactsChanged { facts, .. } => Some(facts.clone()),
        _ => None,
    });
    assert_eq!(facts.unwrap().cli_version.as_deref(), Some("0.0.1"));
}

#[test]
fn a_sidecar_that_dies_turns_the_session_to_error_with_its_stderr() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch_with(fake_sidecar());
    core.add_repository(&repo.path);
    core.dispatch(start(&worktree));
    let id = started_session(&core.wait_for(|seen| has_status(seen, SessionStatus::Waiting)));

    core.dispatch(Command::SendPrompt { session_id: id, text: "crash".into() });
    let events = core.wait_for(|seen| has_status(seen, SessionStatus::Failed));

    let notes: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { entry: Entry::System { text }, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(notes, vec!["The agent process exited (code 3).\nfake sidecar: crashing as asked".to_string()]);
}

#[test]
fn a_sidecar_that_cannot_be_spawned_refuses_the_start() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/x");
    let launcher = Arc::new(SidecarLauncher {
        node: PathBuf::from("/definitely/not/node"),
        script: PathBuf::from("sidecar.mjs"),
        claude: None,
    });
    let mut core = Harness::launch_with(launcher);
    core.add_repository(&repo.path);

    let events = core.start_session(&worktree, "hello");

    assert!(
        matches!(&events[..], [Event::CommandRefused { reason, .. }] if reason.contains("/definitely/not/node")),
        "{events:#?}"
    );
}

#[test]
fn unreadable_sidecar_lines_are_noted_and_written_to_the_compat_log() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch_with(fake_sidecar());
    core.add_repository(&repo.path);
    core.dispatch(start(&worktree));
    let id = started_session(&core.wait_for(|seen| has_status(seen, SessionStatus::Waiting)));

    core.dispatch(Command::SendPrompt { session_id: id, text: "garbage".into() });
    core.wait_for(|seen| has_status(seen, SessionStatus::Waiting));

    let log = core.compat_log();
    let classes: Vec<_> = log.iter().map(|r| r["failure_class"].as_str().unwrap()).collect();
    assert_eq!(classes, vec!["parse_error", "protocol_mismatch"]);
    assert_eq!(log[0]["cli_version"], "0.0.1");
}
