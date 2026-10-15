//! The permission relay through seam 1: an agent's Permission request becomes a pending
//! request card and puts the Session in Needs you; one answer (from any surface: they all
//! send the same command) resolves it, goes back to the agent, and a second is refused.
mod harness;

use std::path::PathBuf;

use adapter_protocol::{Command as AgentCommand, Event as AgentEvent, RequestDetail, Verdict as AgentVerdict};
use harness::{added_repository, recorded_events, started_session, Harness, TempRepo};
use serde_json::json;
use switchyard_schema::{Command, Entry, Event, Request, RequestState, SessionStatus, Verdict};

fn setup() -> (Harness, TempRepo, PathBuf, String) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    let id = started_session(&core.start_session(&worktree, "Run the tests and tidy up."));
    (core, repo, worktree, id)
}

fn command_request(id: &str, command: &str) -> AgentEvent {
    AgentEvent::PermissionRequest {
        request_id: id.into(),
        request: RequestDetail::Command { command: command.into(), cwd: None },
        reason: Some("Run the test suite".into()),
    }
}

fn question(id: &str) -> AgentEvent {
    AgentEvent::PermissionRequest {
        request_id: id.into(),
        request: RequestDetail::Question {
            question: "Should dist/ stay in git?".into(),
            options: vec!["Keep it".into(), "Ignore it".into()],
        },
        reason: None,
    }
}

fn card(id: &str, request: Request, reason: Option<&str>, state: RequestState, answer: Option<&str>) -> Entry {
    Entry::Request {
        request_id: id.into(),
        request,
        reason: reason.map(Into::into),
        state,
        answer: answer.map(Into::into),
    }
}

fn pnpm_test() -> Request {
    Request::Command { command: "pnpm test".into(), cwd: None }
}

fn dist_question() -> Request {
    Request::Question { question: "Should dist/ stay in git?".into(), options: vec!["Keep it".into(), "Ignore it".into()] }
}

fn answer(core: &mut Harness, session: &str, request: &str, verdict: Verdict) -> Vec<Event> {
    core.send(Command::AnswerRequest { session_id: session.into(), request_id: request.into(), verdict })
}

fn answers_sent(core: &Harness) -> Vec<AgentCommand> {
    core.agents.commands(0).into_iter().filter(|c| matches!(c, AgentCommand::AnswerRequest { .. })).collect()
}

fn refused(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CommandRefused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_permission_request_becomes_a_pending_card_and_the_session_needs_you() {
    let (mut core, _repo, _wt, id) = setup();

    let events = core.agent_says(0, command_request("req-1", "pnpm test"));

    assert_eq!(
        events,
        vec![
            Event::TranscriptEntry {
                session_id: id.clone(),
                index: 1,
                entry: card("req-1", pnpm_test(), Some("Run the test suite"), RequestState::Pending, None),
            },
            Event::SessionStatusChanged { session_id: id, status: SessionStatus::Waiting },
        ]
    );
}

#[test]
fn run_once_resolves_the_card_in_place_and_tells_the_agent_once() {
    let (mut core, _repo, _wt, id) = setup();
    core.agent_says(0, command_request("req-1", "pnpm test"));

    let events = answer(&mut core, &id, "req-1", Verdict::AllowOnce);

    assert_eq!(
        events,
        vec![
            Event::TranscriptEntry {
                session_id: id.clone(),
                index: 1,
                entry: card("req-1", pnpm_test(), Some("Run the test suite"), RequestState::Allowed, None),
            },
            Event::SessionStatusChanged { session_id: id.clone(), status: SessionStatus::Running },
        ]
    );
    assert_eq!(
        answers_sent(&core),
        vec![AgentCommand::AnswerRequest { request_id: "req-1".into(), verdict: AgentVerdict::AllowOnce }]
    );

    // A second surface (toast, inbox, card) answering the same request is refused.
    let again = answer(&mut core, &id, "req-1", Verdict::Deny);
    assert_eq!(refused(&again), vec!["That request was already answered".to_string()]);
    assert_eq!(answers_sent(&core).len(), 1);
}

#[test]
fn skip_denies_the_request_and_the_agent_carries_on() {
    let (mut core, _repo, _wt, id) = setup();
    core.agent_says(0, command_request("req-2", "rm -rf dist"));

    let events = answer(&mut core, &id, "req-2", Verdict::Deny);

    assert!(events.contains(&Event::TranscriptEntry {
        session_id: id.clone(),
        index: 1,
        entry: card(
            "req-2",
            Request::Command { command: "rm -rf dist".into(), cwd: None },
            Some("Run the test suite"),
            RequestState::Denied,
            None
        ),
    }));
    assert!(events.contains(&Event::SessionStatusChanged { session_id: id, status: SessionStatus::Running }));
    assert_eq!(
        answers_sent(&core),
        vec![AgentCommand::AnswerRequest { request_id: "req-2".into(), verdict: AgentVerdict::Deny { message: None } }]
    );
}

#[test]
fn a_question_is_answered_by_choosing_an_option() {
    let (mut core, _repo, _wt, id) = setup();
    core.agent_says(0, question("req-3"));

    let events = answer(&mut core, &id, "req-3", Verdict::Answer { text: "Ignore it".into() });

    assert!(events.contains(&Event::TranscriptEntry {
        session_id: id,
        index: 1,
        entry: card("req-3", dist_question(), None, RequestState::Answered, Some("Ignore it")),
    }));
    assert_eq!(
        answers_sent(&core),
        vec![AgentCommand::AnswerRequest {
            request_id: "req-3".into(),
            verdict: AgentVerdict::Answer { text: "Ignore it".into() }
        }]
    );
}

#[test]
fn typing_in_the_composer_answers_a_pending_question() {
    let (mut core, _repo, _wt, id) = setup();
    core.agent_says(0, question("req-3"));

    let events = core.send(Command::SendPrompt { session_id: id.clone(), text: "Ignore it, and add a note".into() });

    assert!(events.contains(&Event::TranscriptEntry {
        session_id: id.clone(),
        index: 1,
        entry: card("req-3", dist_question(), None, RequestState::Answered, Some("Ignore it, and add a note")),
    }));
    assert!(events.contains(&Event::SessionStatusChanged { session_id: id, status: SessionStatus::Running }));
    let sent = core.agents.commands(0);
    assert_eq!(
        sent.last(),
        Some(&AgentCommand::AnswerRequest {
            request_id: "req-3".into(),
            verdict: AgentVerdict::Answer { text: "Ignore it, and add a note".into() }
        })
    );
    assert!(!sent.contains(&AgentCommand::SendText { text: "Ignore it, and add a note".into() }));
}

#[test]
fn a_prompt_cannot_jump_a_pending_permission_request() {
    let (mut core, _repo, _wt, id) = setup();
    core.agent_says(0, command_request("req-1", "pnpm test"));

    let events = core.send(Command::SendPrompt { session_id: id, text: "go on".into() });

    assert_eq!(refused(&events), vec!["Answer the agent's request first: Run once or Skip".to_string()]);
    assert_eq!(core.agents.commands(0).len(), 1, "only the first prompt reached the agent");
}

#[test]
fn verdicts_must_fit_the_request() {
    let (mut core, _repo, _wt, id) = setup();
    core.agent_says(0, command_request("req-1", "pnpm test"));
    core.agent_says(0, question("req-3"));

    let wrong = [
        answer(&mut core, &id, "req-1", Verdict::Answer { text: "yes".into() }),
        answer(&mut core, &id, "req-3", Verdict::AllowOnce),
        answer(&mut core, &id, "nope", Verdict::AllowOnce),
    ];

    assert_eq!(
        wrong.iter().flat_map(|e| refused(e)).collect::<Vec<_>>(),
        vec![
            "Only a question takes an answer".to_string(),
            "A question needs an answer".to_string(),
            "There is no request nope in this session".to_string(),
        ]
    );
    assert!(answers_sent(&core).is_empty());
}

#[test]
fn the_session_needs_you_until_every_pending_request_is_answered() {
    let (mut core, _repo, _wt, id) = setup();
    core.agent_says(0, command_request("req-1", "pnpm test"));
    core.agent_says(0, command_request("req-2", "pnpm lint"));

    let first = answer(&mut core, &id, "req-1", Verdict::AllowOnce);
    let second = answer(&mut core, &id, "req-2", Verdict::AllowOnce);

    let status = |events: &[Event]| {
        events.iter().find_map(|e| match e {
            Event::SessionStatusChanged { status, .. } => Some(*status),
            _ => None,
        })
    };
    assert_eq!(status(&first), None, "still Needs you: req-2 is pending");
    assert_eq!(status(&second), Some(SessionStatus::Running));
}

#[test]
fn stop_cancels_every_pending_request() {
    let (mut core, _repo, _wt, id) = setup();
    core.agent_says(0, command_request("req-1", "pnpm test"));

    let events = core.send(Command::StopSession { session_id: id.clone() });

    assert!(events.contains(&Event::TranscriptEntry {
        session_id: id.clone(),
        index: 1,
        entry: card("req-1", pnpm_test(), Some("Run the test suite"), RequestState::Cancelled, None),
    }));
    assert!(events.contains(&Event::SessionStatusChanged { session_id: id.clone(), status: SessionStatus::Idle }));
    assert_eq!(core.agents.commands(0).last(), Some(&AgentCommand::Interrupt));
    assert_eq!(refused(&answer(&mut core, &id, "req-1", Verdict::AllowOnce)), vec!["That request was cancelled (the session stopped or Switchyard restarted); nothing was sent to the agent".to_string()]);
    assert!(answers_sent(&core).is_empty());
}

#[test]
fn a_request_arriving_after_stop_is_declined_unseen() {
    let (mut core, _repo, _wt, id) = setup();
    core.send(Command::StopSession { session_id: id.clone() });

    let events = core.agent_says(0, command_request("req-9", "pnpm test"));

    assert_eq!(
        events,
        vec![Event::TranscriptEntry {
            session_id: id,
            index: 2,
            entry: card("req-9", pnpm_test(), Some("Run the test suite"), RequestState::Cancelled, None),
        }]
    );
    assert_eq!(
        answers_sent(&core),
        vec![AgentCommand::AnswerRequest {
            request_id: "req-9".into(),
            verdict: AgentVerdict::Deny { message: Some("The session was stopped.".into()) }
        }]
    );
}

#[test]
fn the_five_kinds_reach_the_transcript() {
    let (mut core, _repo, _wt, id) = setup();
    let kinds = [
        (RequestDetail::FileWrite { path: "README.md".into(), diff: Some("+hi".into()) }, Request::FileWrite {
            path: "README.md".into(),
            diff: Some("+hi".into()),
        }),
        (
            RequestDetail::Network { domain: "docs.example.com".into(), url: None },
            Request::Network { domain: "docs.example.com".into(), url: None },
        ),
        (
            RequestDetail::McpTool { server: "github".into(), tool: "create_issue".into(), input: json!({"title": "x"}) },
            Request::McpTool { server: "github".into(), tool: "create_issue".into(), input: r#"{"title":"x"}"#.into() },
        ),
    ];
    for (n, (detail, expected)) in kinds.into_iter().enumerate() {
        let request_id = format!("k{n}");
        let events = core.agent_says(
            0,
            AgentEvent::PermissionRequest { request_id: request_id.clone(), request: detail, reason: None },
        );
        assert_eq!(
            events[0],
            Event::TranscriptEntry {
                session_id: id.clone(),
                index: 1 + n as u32,
                entry: card(&request_id, expected, None, RequestState::Pending, None),
            }
        );
    }
}

#[test]
fn a_recorded_relay_session_replays_through_the_core() {
    let (mut core, _repo, _wt, id) = setup();
    let recording = recorded_events("claude-code/permission-relay");
    let at = |request: &str| {
        recording
            .iter()
            .position(|e| matches!(e, AgentEvent::PermissionRequest { request_id, .. } if request_id == request))
            .unwrap()
    };

    core.agent_replays(0, &recording[..=at("req-1")]);
    answer(&mut core, &id, "req-1", Verdict::AllowOnce);
    core.agent_replays(0, &recording[at("req-1") + 1..=at("req-2")]);
    answer(&mut core, &id, "req-2", Verdict::Deny);
    core.agent_replays(0, &recording[at("req-2") + 1..=at("req-3")]);
    answer(&mut core, &id, "req-3", Verdict::Answer { text: "Ignore it".into() });
    let end = core.agent_replays(0, &recording[at("req-3") + 1..]);

    assert_eq!(
        answers_sent(&core),
        vec![
            AgentCommand::AnswerRequest { request_id: "req-1".into(), verdict: AgentVerdict::AllowOnce },
            AgentCommand::AnswerRequest { request_id: "req-2".into(), verdict: AgentVerdict::Deny { message: None } },
            AgentCommand::AnswerRequest {
                request_id: "req-3".into(),
                verdict: AgentVerdict::Answer { text: "Ignore it".into() }
            },
        ]
    );
    assert!(end.contains(&Event::SessionStatusChanged { session_id: id, status: SessionStatus::Waiting }));
}
