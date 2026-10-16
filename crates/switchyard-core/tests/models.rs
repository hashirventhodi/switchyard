//! A Session's Model through seam 1: chosen at start (a Model id, or Default, which passes
//! nothing to the CLI), never hidden once it runs (the Agent log names what Default
//! resolved to), and usage on an unresolved Default held unpriced until a concrete Model
//! name resolves it.
mod harness;

use std::path::PathBuf;

use adapter_protocol::{CliInfo, Event as AgentEvent, PermissionMode as ProtocolMode};
use harness::{added_repository, recorded_events, started_session, Harness, TempRepo};
use switchyard_schema::{AgentKind, Command, Entry, Event, ModelChoice, PermissionMode, SessionFacts};

fn setup() -> (Harness, TempRepo, PathBuf) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    (core, repo, worktree)
}

fn model(id: &str) -> ModelChoice {
    ModelChoice::Model { id: id.into() }
}

#[test]
fn a_session_started_on_a_chosen_model_runs_its_cli_on_that_model() {
    let (mut core, _repo, worktree) = setup();

    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, model("claude-sonnet-5-5"), "Fix it."));

    assert_eq!(core.agents.launches()[0].model.as_deref(), Some("claude-sonnet-5-5"));
}

#[test]
fn a_session_started_on_default_passes_no_model_to_its_cli() {
    let (mut core, _repo, worktree) = setup();

    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, ModelChoice::Default, "Fix it."));

    assert_eq!(core.agents.launches()[0].model, None);
}

fn init(model: Option<&str>) -> AgentEvent {
    AgentEvent::SessionInit {
        external_session_id: "cli-1".into(),
        permission_mode: ProtocolMode::AskFirst,
        cli_permission_mode: "default".into(),
        cli: CliInfo { name: "claude-code".into(), version: "2.1.300".into() },
        model: model.map(str::to_string),
        mcp_servers: vec![],
    }
}

/// The text of every system line (Agent log `#` line) in `events`.
fn system_lines(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { entry: Entry::System { text }, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The Session's facts as last reported in `events`.
fn last_facts(events: &[Event]) -> SessionFacts {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionFactsChanged { facts, .. } => Some(facts.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no facts in {events:#?}"))
}

#[test]
fn the_agent_log_names_what_default_resolved_to_at_start() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));

    let events = core.agent_replays(0, &recorded_events("claude-code/first-session"));

    assert_eq!(system_lines(&events), vec!["Model: Default → claude-opus-5-5".to_string()]);
}

#[test]
fn the_agent_log_names_the_chosen_model_at_start() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, model("claude-sonnet-5-5"), "Fix it."));

    let events = core.agent_says(0, init(Some("claude-sonnet-5-5")));

    assert_eq!(system_lines(&events), vec!["Model: claude-sonnet-5-5".to_string()]);
    assert_eq!(last_facts(&events).model.as_deref(), Some("claude-sonnet-5-5"));
}

#[test]
fn a_default_the_cli_does_not_report_is_logged_as_not_reported_and_never_guessed() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));

    let events = core.agent_says(0, init(None));

    assert_eq!(system_lines(&events), vec!["Model: Default (not reported)".to_string()]);
    assert_eq!(last_facts(&events).model, None);
}

#[test]
fn a_chosen_model_the_cli_does_not_report_is_the_one_it_was_started_on() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, model("claude-sonnet-5-5"), "Fix it."));

    let events = core.agent_says(0, init(None));

    assert_eq!(system_lines(&events), vec!["Model: claude-sonnet-5-5".to_string()]);
    assert_eq!(last_facts(&events).model.as_deref(), Some("claude-sonnet-5-5"));
}

#[test]
fn an_explore_session_starts_on_its_chosen_model_too() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session_on(&worktree, PermissionMode::Explore, model("claude-haiku-4-5"), "Look around."));

    let events = core.agent_says(0, init(Some("claude-haiku-4-5")));

    assert_eq!(core.agents.launches()[0].model.as_deref(), Some("claude-haiku-4-5"));
    assert_eq!(system_lines(&events), vec!["Model: claude-haiku-4-5".to_string()]);
}

#[test]
fn the_model_line_is_written_once_and_a_resumed_cli_does_not_repeat_it() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init(Some("claude-opus-5-5")));
    core.agent_says(0, AgentEvent::TurnComplete);
    core.agent_exits(0, Some(0), "");

    core.send(Command::SendPrompt { session_id: id, text: "Go on.".into() });
    assert_eq!(core.agents.launches().len(), 2, "the gone agent is relaunched by its id");
    let events = core.agent_says(1, init(Some("claude-opus-5-5")));

    assert_eq!(system_lines(&events), Vec::<String>::new());
}

#[test]
fn archived_sessions_keep_their_model_line() {
    let (mut core, _repo, worktree) = setup();
    let old = started_session(&core.start_session(&worktree, "One"));
    core.agent_says(0, init(Some("claude-opus-5-5")));
    core.send(Command::StopSession { session_id: old.clone() });
    core.send(Command::ReplaceSession {
        session_id: old.clone(),
        agent: AgentKind::ClaudeCode,
        permission_mode: PermissionMode::AskFirst,
        model: model("claude-sonnet-5-5"),
        prompt: "Two".into(),
    });
    assert_eq!(core.agents.launches()[1].model.as_deref(), Some("claude-sonnet-5-5"));

    let read = core.send(Command::ReadArchivedSession { session_id: old.clone() });

    let entries = read
        .iter()
        .find_map(|e| match e {
            Event::ArchivedTranscript { entries, .. } => Some(entries.clone()),
            _ => None,
        })
        .unwrap();
    assert!(entries.contains(&Entry::System { text: "Model: Default → claude-opus-5-5".into() }), "{entries:#?}");
}
