//! Switching a running Session's Model through seam 1: the user's switch is pending until
//! the agent's own requests are confirmed on the new Model (`model_changed`), or fails (the
//! CLI refused it, or the next request was still on the old Model) with a note, the control
//! back on the running Model and the status untouched. A switch made while a Permission
//! request is pending waits for the answer, so its one-request check starts after it.
mod harness;

use std::path::{Path, PathBuf};

use adapter_protocol::{
    CliInfo, Command as AgentCommand, Event as AgentEvent, ListedModel, PermissionMode as ProtocolMode, RequestDetail,
};
use harness::{added_repository, started_session, Harness, TempRepo};
use switchyard_schema::{AgentKind, Command, Entry, Event, PermissionMode, Session, SessionFacts, SessionStatus, Verdict};

const OPUS: &str = "claude-opus-5-5";
const SONNET: &str = "claude-sonnet-5-5";

fn setup() -> (Harness, TempRepo, PathBuf) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    (core, repo, worktree)
}

/// A Claude Code Session running on Opus.
fn running_on_opus(core: &mut Harness, worktree: &Path, mode: PermissionMode) -> String {
    let id = started_session(&core.start_session_in(worktree, mode, "Fix it."));
    core.agent_says(
        0,
        AgentEvent::SessionInit {
            external_session_id: "cli-1".into(),
            permission_mode: ProtocolMode::AskFirst,
            cli_permission_mode: "default".into(),
            cli: CliInfo { name: "claude-code".into(), version: "2.1.300".into() },
            model: Some(OPUS.into()),
            mcp_servers: vec![],
        },
    );
    id
}

fn switch(core: &mut Harness, id: &str, model: &str) -> Vec<Event> {
    core.send(Command::SetModel { session_id: id.into(), model: model.into() })
}

/// The Session's switch state as last reported: (pending, last confirmed switch).
fn switch_state(events: &[Event]) -> Option<(Option<String>, Option<String>)> {
    events.iter().rev().find_map(|e| match e {
        Event::SessionModelChanged { pending_model, switched_model, .. } => {
            Some((pending_model.clone(), switched_model.clone()))
        }
        _ => None,
    })
}

fn system_lines(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { entry: Entry::System { text }, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn last_facts(events: &[Event]) -> Option<SessionFacts> {
    events.iter().rev().find_map(|e| match e {
        Event::SessionFactsChanged { facts, .. } => Some(facts.clone()),
        _ => None,
    })
}

fn set_models(core: &Harness) -> Vec<String> {
    core.agents
        .commands(0)
        .into_iter()
        .filter_map(|c| match c {
            AgentCommand::SetModel { model } => Some(model),
            _ => None,
        })
        .collect()
}

#[test]
fn a_switch_is_pending_until_the_agents_own_requests_run_on_the_new_model() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);

    let asked = switch(&mut core, &id, SONNET);

    assert_eq!(switch_state(&asked), Some((Some(SONNET.into()), None)));
    assert_eq!(set_models(&core), vec![SONNET.to_string()], "the CLI is told at once");

    let confirmed = core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });

    assert_eq!(switch_state(&confirmed), Some((None, Some(SONNET.into()))));
    assert_eq!(last_facts(&confirmed).and_then(|f| f.model).as_deref(), Some(SONNET));
    assert_eq!(system_lines(&confirmed), vec![format!("Model → {SONNET}")]);
}

fn failed_switch(events: &[Event]) -> Option<(String, Option<String>)> {
    events.iter().find_map(|e| match e {
        Event::ModelSwitchFailed { model, reason, .. } => Some((model.clone(), reason.clone())),
        _ => None,
    })
}

#[test]
fn a_switch_the_cli_refuses_fails_with_its_reason_and_leaves_the_session_as_it_was() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);
    switch(&mut core, &id, "claude-nope");

    let events = core.agent_says(
        0,
        AgentEvent::ModelChangeFailed { model: "claude-nope".into(), reason: Some("Model 'claude-nope' not found".into()) },
    );

    assert_eq!(failed_switch(&events), Some(("claude-nope".into(), Some("Model 'claude-nope' not found".into()))));
    assert_eq!(switch_state(&events), Some((None, None)), "the control reverts to the running Model");
    assert_eq!(system_lines(&events), vec!["Model switch to claude-nope failed: Model 'claude-nope' not found".to_string()]);
    assert!(!events.iter().any(|e| matches!(e, Event::SessionStatusChanged { .. })), "{events:#?}");
    let attach = core.send(Command::Attach);
    assert_eq!(last_facts(&attach).and_then(|f| f.model).as_deref(), Some(OPUS));
    assert_eq!(set_models(&core).len(), 1, "never retried");
}

#[test]
fn a_switch_whose_next_request_is_still_on_the_old_model_fails_saying_so() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);
    switch(&mut core, &id, SONNET);

    let events = core.agent_says(0, AgentEvent::ModelChangeFailed { model: SONNET.into(), reason: None });

    let why = format!("the agent's next request was still on {OPUS}");
    assert_eq!(failed_switch(&events), Some((SONNET.into(), Some(why))), "the core words why");
    assert_eq!(switch_state(&events), Some((None, None)));
    assert_eq!(
        system_lines(&events),
        vec![format!("Model switch to {SONNET} did not take: the agent's next request was still on {OPUS}")]
    );
}

/// The agent asks to run a command: the Session is Waiting on a Permission request.
fn asks_permission(core: &mut Harness) {
    core.agent_says(
        0,
        AgentEvent::PermissionRequest {
            request_id: "req-1".into(),
            request: RequestDetail::Command { command: "pnpm test".into(), cwd: None },
            reason: None,
        },
    );
}

/// The switch and answer commands the agent got, in order.
fn switch_and_answers(core: &Harness) -> Vec<String> {
    core.agents
        .commands(0)
        .into_iter()
        .filter_map(|c| match c {
            AgentCommand::SetModel { model } => Some(format!("set_model {model}")),
            AgentCommand::AnswerRequest { request_id, .. } => Some(format!("answer {request_id}")),
            _ => None,
        })
        .collect()
}

#[test]
fn a_switch_made_while_a_permission_request_is_pending_waits_for_the_answer() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);
    asks_permission(&mut core);

    let asked = switch(&mut core, &id, SONNET);

    assert_eq!(switch_state(&asked), Some((Some(SONNET.into()), None)), "shown as pending at once");
    assert!(switch_and_answers(&core).is_empty(), "the pending request is left exactly as it is");

    let answered =
        core.send(Command::AnswerRequest { session_id: id.clone(), request_id: "req-1".into(), verdict: Verdict::AllowOnce });

    // Sent just before the answer, so the request after the answer is the one checked.
    assert_eq!(switch_and_answers(&core), vec![format!("set_model {SONNET}"), "answer req-1".to_string()]);
    assert!(
        answered.iter().any(|e| matches!(e, Event::SessionStatusChanged { status: SessionStatus::Running, .. })),
        "{answered:#?}"
    );
    let confirmed = core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });
    assert_eq!(switch_state(&confirmed), Some((None, Some(SONNET.into()))));
}

#[test]
fn switching_back_to_the_running_model_withdraws_a_pending_switch() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);
    switch(&mut core, &id, SONNET);

    let events = switch(&mut core, &id, OPUS);

    assert_eq!(switch_state(&events), Some((None, None)), "nothing left to confirm");
    assert_eq!(set_models(&core), vec![SONNET.to_string(), OPUS.to_string()], "the CLI is told to stay");
    assert!(system_lines(&events).is_empty(), "{events:#?}");
}

#[test]
fn switching_to_the_model_already_running_does_nothing() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);

    let events = switch(&mut core, &id, OPUS);

    assert_eq!(switch_state(&events), None);
    assert!(set_models(&core).is_empty());
}

/// The CLI lists its Models by alias, as Claude Code does, each with the full id it stands for.
fn lists_aliases(core: &mut Harness) {
    let alias = |id: &str, resolved: &str| ListedModel {
        id: id.into(),
        display_name: id.to_uppercase(),
        resolved_model: Some(resolved.into()),
    };
    core.agent_says(0, AgentEvent::ModelsListed { models: vec![alias("opus", OPUS), alias("sonnet", SONNET)] });
}

#[test]
fn switching_to_an_alias_of_the_model_already_running_does_nothing() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);
    lists_aliases(&mut core);

    let events = switch(&mut core, &id, "opus");

    assert_eq!(switch_state(&events), None, "opus is the Model running: nothing to switch");
    assert!(set_models(&core).is_empty());
}

#[test]
fn a_switch_named_by_an_alias_is_confirmed_by_the_full_id_it_stands_for() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);
    lists_aliases(&mut core);
    switch(&mut core, &id, "sonnet");

    let confirmed = core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });

    assert_eq!(switch_state(&confirmed), Some((None, Some(SONNET.into()))));
    assert_eq!(system_lines(&confirmed), vec![format!("Model → {SONNET}")]);
}

#[test]
fn the_agent_moving_to_another_model_on_its_own_never_changes_the_sessions_model() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);

    let events = core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });

    assert_eq!(switch_state(&events), None, "not a switch the user made");
    assert_eq!(
        system_lines(&events),
        vec![format!("The agent moved itself to {SONNET}: not your switch, so the Session's Model stays {OPUS}")]
    );
    let attach = core.send(Command::Attach);
    assert_eq!(last_facts(&attach).and_then(|f| f.model).as_deref(), Some(OPUS));
    assert_eq!(replayed_session(&attach, &id).switched_model, None);
}

#[test]
fn a_model_change_to_another_model_than_the_pending_switch_fails_that_switch() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);
    switch(&mut core, &id, SONNET);

    let events = core.agent_says(0, AgentEvent::ModelChanged { model: "claude-haiku-4-5".into() });

    let why = "the agent's next request ran on claude-haiku-4-5".to_string();
    assert_eq!(failed_switch(&events), Some((SONNET.into(), Some(why.clone()))));
    assert_eq!(switch_state(&events), Some((None, None)), "the control is back on the Session's Model");
    assert_eq!(system_lines(&events), vec![format!("Model switch to {SONNET} did not take: {why}")]);
    let attach = core.send(Command::Attach);
    assert_eq!(last_facts(&attach).and_then(|f| f.model).as_deref(), Some(OPUS));
}

fn replayed_session(events: &[Event], id: &str) -> Session {
    events
        .iter()
        .find_map(|e| match e {
            Event::SessionStarted { session } if session.id == id => Some(session.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{id} not replayed in {events:#?}"))
}

#[test]
fn a_pending_switch_and_the_last_confirmed_one_outlive_a_relaunch() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);
    switch(&mut core, &id, SONNET);
    core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });
    switch(&mut core, &id, "claude-haiku-4-5");

    let mut core = core.relaunch();
    let session = replayed_session(&core.send(Command::Attach), &id);

    assert_eq!(session.switched_model.as_deref(), Some(SONNET));
    assert_eq!(session.pending_model.as_deref(), Some("claude-haiku-4-5"));
}

#[test]
fn the_switch_lines_are_kept_in_the_archived_sessions_transcript() {
    let (mut core, _repo, worktree) = setup();
    let id = running_on_opus(&mut core, &worktree, PermissionMode::AskFirst);
    switch(&mut core, &id, SONNET);
    core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });
    switch(&mut core, &id, "claude-nope");
    core.agent_says(0, AgentEvent::ModelChangeFailed { model: "claude-nope".into(), reason: Some("not found".into()) });
    core.send(Command::StopSession { session_id: id.clone() });
    core.send(Command::ReplaceSession {
        session_id: id.clone(),
        agent: AgentKind::ClaudeCode,
        permission_mode: PermissionMode::AskFirst,
        model: Default::default(),
        prompt: "Again.".into(),
    });

    let read = core.send(Command::ReadArchivedSession { session_id: id.clone() });
    let lines: Vec<String> = read
        .iter()
        .find_map(|e| match e {
            Event::ArchivedTranscript { entries, .. } => Some(entries.clone()),
            _ => None,
        })
        .expect("archived transcript")
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::System { text } if text.starts_with("Model") => Some(text),
            _ => None,
        })
        .collect();

    assert_eq!(
        lines,
        vec![
            format!("Model: Default → {OPUS}"),
            format!("Model → {SONNET}"),
            "Model switch to claude-nope failed: not found".to_string()
        ]
    );
}

#[test]
fn a_switch_works_the_same_in_an_explore_session() {
    let (mut core, repo, _worktree) = setup();
    let id = running_on_opus(&mut core, &repo.path, PermissionMode::Explore);

    switch(&mut core, &id, SONNET);
    let confirmed = core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });

    assert_eq!(set_models(&core), vec![SONNET.to_string()]);
    assert_eq!(switch_state(&confirmed), Some((None, Some(SONNET.into()))));
}
