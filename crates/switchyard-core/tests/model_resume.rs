//! Resuming a Session on its current Model, through seam 1: a relaunched agent resumes on
//! the last confirmed switch, else the queued switch, else the start choice. A switch
//! applied on resume is confirmed or failed by the resumed CLI's first request, and a resume
//! the CLI cannot run on its Model falls back to Default once, with a note.
mod harness;

use std::path::PathBuf;

use adapter_protocol::{CliInfo, Command as AgentCommand, Event as AgentEvent, PermissionMode as ProtocolMode};
use harness::{added_repository, started_session, Harness, TempRepo};
use switchyard_schema::{Command, Entry, Event, ModelChoice, PermissionMode};

const OPUS: &str = "claude-opus-5-5";
const SONNET: &str = "claude-sonnet-5-5";

fn setup() -> (Harness, TempRepo, PathBuf) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    (core, repo, worktree)
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

/// A Claude Code Session started on `choice`, its CLI running on Opus.
fn started_on(core: &mut Harness, worktree: &PathBuf, choice: ModelChoice) -> String {
    let id = started_session(&core.start_session_on(worktree, PermissionMode::AskFirst, choice, "Fix it."));
    core.agent_says(0, init(Some(OPUS)));
    id
}

fn switch(core: &mut Harness, id: &str, model: &str) -> Vec<Event> {
    core.send(Command::SetModel { session_id: id.into(), model: model.into() })
}

/// Switchyard closed and opened again: the Session is Paused, its agent gone.
fn reopened(core: Harness) -> Harness {
    let mut core = core.relaunch();
    core.send(Command::Attach);
    core
}

fn resume(core: &mut Harness, id: &str) -> Vec<Event> {
    core.send(Command::ResumeSession { session_id: id.into() })
}

/// The Model the n-th agent was launched and told to resume on (they must agree).
fn resumed_on(core: &Harness, agent: usize) -> Option<String> {
    let launched = core.agents.launches()[agent].model.clone();
    match &core.agents.commands(agent)[0] {
        AgentCommand::Resume { model, .. } => {
            assert_eq!(*model, launched, "the launch and the resume name the same Model");
            model.clone()
        }
        other => panic!("not a resume: {other:?}"),
    }
}

fn set_models(core: &Harness, agent: usize) -> Vec<String> {
    core.agents
        .commands(agent)
        .into_iter()
        .filter_map(|c| match c {
            AgentCommand::SetModel { model } => Some(model),
            _ => None,
        })
        .collect()
}

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

#[test]
fn an_idle_session_resumes_on_the_model_it_started_on() {
    let (mut core, _repo, worktree) = setup();
    let id = started_on(&mut core, &worktree, ModelChoice::Model { id: OPUS.into() });

    let mut core = reopened(core);
    resume(&mut core, &id);

    assert_eq!(resumed_on(&core, 0).as_deref(), Some(OPUS));
}

#[test]
fn a_session_started_on_default_resumes_on_default() {
    let (mut core, _repo, worktree) = setup();
    let id = started_on(&mut core, &worktree, ModelChoice::Default);

    let mut core = reopened(core);
    resume(&mut core, &id);

    assert_eq!(resumed_on(&core, 0), None);
}

#[test]
fn an_idle_session_resumes_on_its_last_confirmed_switch() {
    let (mut core, _repo, worktree) = setup();
    let id = started_on(&mut core, &worktree, ModelChoice::Model { id: OPUS.into() });
    switch(&mut core, &id, SONNET);
    core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });

    let mut core = reopened(core);
    resume(&mut core, &id);

    assert_eq!(resumed_on(&core, 0).as_deref(), Some(SONNET));
    assert!(set_models(&core, 0).is_empty(), "nothing left to switch");
}

#[test]
fn a_switch_queued_while_paused_is_the_model_it_resumes_on_and_its_first_request_confirms_it() {
    let (mut core, _repo, worktree) = setup();
    let id = started_on(&mut core, &worktree, ModelChoice::Model { id: OPUS.into() });
    let mut core = reopened(core);
    let asked = switch(&mut core, &id, SONNET);
    assert_eq!(switch_state(&asked), Some((Some(SONNET.into()), None)), "pending while paused");

    resume(&mut core, &id);

    assert_eq!(resumed_on(&core, 0).as_deref(), Some(SONNET));
    assert!(set_models(&core, 0).is_empty(), "applied by the resume, not switched live");
    let init = core.agent_says(0, init(Some(SONNET)));
    assert_eq!(switch_state(&init), None, "still pending: no request has run yet");
    let first = core.agent_says(0, AgentEvent::AgentText { text: "Picking up.".into() });
    assert_eq!(switch_state(&first), Some((None, Some(SONNET.into()))));
    assert_eq!(system_lines(&first), vec![format!("Model → {SONNET}")]);
}

#[test]
fn a_switch_applied_on_resume_fails_when_the_resumed_cli_is_still_on_the_old_model() {
    let (mut core, _repo, worktree) = setup();
    let id = started_on(&mut core, &worktree, ModelChoice::Model { id: OPUS.into() });
    let mut core = reopened(core);
    switch(&mut core, &id, SONNET);
    resume(&mut core, &id);

    core.agent_says(0, init(Some(OPUS)));
    let first = core.agent_says(0, AgentEvent::AgentText { text: "Picking up.".into() });

    assert_eq!(switch_state(&first), Some((None, None)));
    assert!(
        first.iter().any(|e| matches!(e, Event::ModelSwitchFailed { model, reason: None, .. } if model == SONNET)),
        "{first:#?}"
    );
    assert_eq!(
        system_lines(&first),
        vec![format!("Model switch to {SONNET} did not take: the agent's next request was still on {OPUS}")]
    );
}

#[test]
fn a_switch_queued_after_a_confirmed_one_goes_live_once_the_cli_resumes_on_the_confirmed_one() {
    let (mut core, _repo, worktree) = setup();
    let id = started_on(&mut core, &worktree, ModelChoice::Model { id: OPUS.into() });
    switch(&mut core, &id, SONNET);
    core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });
    let mut core = reopened(core);
    switch(&mut core, &id, "claude-haiku-4-5");

    resume(&mut core, &id);

    assert_eq!(resumed_on(&core, 0).as_deref(), Some(SONNET));
    assert_eq!(set_models(&core, 0), vec!["claude-haiku-4-5".to_string()], "checked like any live switch");
}

/// Claude Code's adapter now says it cannot switch its CLI's Model live.
fn cannot_switch_live(core: &mut Harness) {
    core.probe.switches_model_live(false);
    core.send(Command::CheckAgents);
}

fn stop(core: &mut Harness, id: &str) {
    core.send(Command::StopSession { session_id: id.into() });
}

#[test]
fn on_a_cli_that_cannot_switch_live_a_switch_leaves_the_running_process_alone_and_applies_on_resume() {
    let (mut core, _repo, worktree) = setup();
    let id = started_on(&mut core, &worktree, ModelChoice::Model { id: OPUS.into() });
    cannot_switch_live(&mut core);

    let asked = switch(&mut core, &id, SONNET);

    assert_eq!(switch_state(&asked), Some((Some(SONNET.into()), None)));
    assert!(set_models(&core, 0).is_empty(), "the running process is untouched");
    stop(&mut core, &id);
    resume(&mut core, &id);
    assert_eq!(core.agents.launches().len(), 2, "relaunched although its process still ran");
    assert_eq!(resumed_on(&core, 1).as_deref(), Some(SONNET));
    assert!(set_models(&core, 1).is_empty());
    let replaced = core.agent_exits(0, None, "");
    assert!(replaced.is_empty(), "the replaced process has nothing more to say: {replaced:#?}");
    core.agent_says(1, init(Some(SONNET)));
    let first = core.agent_says(1, AgentEvent::TurnComplete);
    assert_eq!(switch_state(&first), Some((None, Some(SONNET.into()))));
}

#[test]
fn on_a_cli_that_cannot_switch_live_the_pending_switch_wins_over_an_earlier_confirmed_one() {
    let (mut core, _repo, worktree) = setup();
    let id = started_on(&mut core, &worktree, ModelChoice::Model { id: OPUS.into() });
    cannot_switch_live(&mut core);
    switch(&mut core, &id, SONNET);
    stop(&mut core, &id);
    resume(&mut core, &id);
    core.agent_says(1, init(Some(SONNET)));
    core.agent_says(1, AgentEvent::TurnComplete);

    switch(&mut core, &id, "claude-haiku-4-5");
    stop(&mut core, &id);
    resume(&mut core, &id);

    assert_eq!(resumed_on(&core, 2).as_deref(), Some("claude-haiku-4-5"), "it could not be applied any other way");
}

#[test]
fn a_switch_sent_to_an_agent_that_then_exited_is_sent_again_to_the_relaunched_one() {
    let (mut core, _repo, worktree) = setup();
    let id = started_on(&mut core, &worktree, ModelChoice::Model { id: OPUS.into() });
    switch(&mut core, &id, SONNET);
    core.agent_says(0, AgentEvent::ModelChanged { model: SONNET.into() });
    switch(&mut core, &id, "claude-haiku-4-5");
    core.agent_exits(0, Some(1), "");

    core.send(Command::SendPrompt { session_id: id.clone(), text: "Go on.".into() });

    assert_eq!(resumed_on(&core, 1).as_deref(), Some(SONNET));
    assert_eq!(set_models(&core, 1), vec!["claude-haiku-4-5".to_string()]);
}
