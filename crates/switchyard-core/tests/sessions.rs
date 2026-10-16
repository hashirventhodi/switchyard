mod harness;

use std::path::PathBuf;

use harness::{added_repository, recorded_events, started_session, Harness, TempRepo, NOW_MS};
use adapter_protocol::{Command as AgentCommand, Event as AgentEvent};
use switchyard_core::agents::LaunchSpec;
use switchyard_schema::{
    AgentKind, Command, Entry, Event, PermissionMode, Session, SessionFacts, SessionStatus, ToolState,
};

/// A repository with one linked worktree on `fix/readme`, added to a fresh core.
fn setup() -> (Harness, TempRepo, PathBuf, String) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));
    (core, repo, worktree, repository.id)
}

#[test]
fn starting_a_session_launches_claude_code_in_the_worktree_and_sends_the_prompt() {
    let (mut core, _repo, worktree, repository_id) = setup();

    let events = core.start_session(&worktree, "Fix the broken link in the README.");

    let id = started_session(&events);
    assert_eq!(
        events,
        vec![
            Event::SessionStarted {
                session: Session {
                    id: id.clone(),
                    repository_id,
                    worktree_path: worktree.to_str().unwrap().into(),
                    branch: Some("fix/readme".into()),
                    agent: AgentKind::ClaudeCode,
                    permission_mode: PermissionMode::AskFirst,
                    model: Default::default(),
                    status: SessionStatus::Running,
                    started_at_ms: NOW_MS,
                },
            },
            Event::TranscriptEntry {
                session_id: id,
                index: 0,
                entry: Entry::User { text: "Fix the broken link in the README.".into() },
            },
        ]
    );
    assert_eq!(
        core.agents.launches(),
        vec![LaunchSpec { agent: AgentKind::ClaudeCode, cwd: worktree, permission_mode: PermissionMode::AskFirst, model: None, sandboxed: false, allowed_tools: vec![], disallowed_tools: vec![], mcp_servers: vec![], env: Default::default() }]
    );
    assert_eq!(
        core.agents.commands(0),
        vec![adapter_protocol::Command::SendText { text: "Fix the broken link in the README.".into() }]
    );
}

fn statuses(events: &[Event]) -> Vec<SessionStatus> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::SessionStatusChanged { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
}

fn entries(events: &[Event]) -> Vec<(u32, Entry)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { index, entry, .. } => Some((*index, entry.clone())),
            _ => None,
        })
        .collect()
}

fn tool(call: &str, tool: &str, summary: &str, state: ToolState, output: Option<&str>) -> Entry {
    Entry::Tool {
        call_id: call.into(),
        tool: tool.into(),
        summary: summary.into(),
        state,
        output: output.map(Into::into),
    }
}

#[test]
fn a_recorded_session_streams_into_the_transcript_and_walks_needs_you_then_finished() {
    let (mut core, _repo, worktree, _) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix the broken link in the README."));
    let recording = recorded_events("claude-code/first-session");
    let first_turn_end = recording.iter().position(|e| *e == AgentEvent::TurnComplete).unwrap();

    let first_turn = core.agent_replays(0, &recording[..=first_turn_end]);

    assert_eq!(
        entries(&first_turn),
        vec![
            (1, Entry::System { text: "Model: Default → claude-opus-5-5".into() }),
            (2, Entry::Agent { text: "I'll look at the README and the test script.".into() }),
            (3, tool("toolu_01", "Bash", "ls", ToolState::Running, None)),
            (3, tool("toolu_01", "Bash", "ls", ToolState::Ok, Some("README.md\npackage.json\nsrc"))),
            (4, tool("toolu_02", "Read", "/wt/acme/fix-readme/docs/missing.md", ToolState::Running, None)),
            (
                4,
                tool(
                    "toolu_02",
                    "Read",
                    "/wt/acme/fix-readme/docs/missing.md",
                    ToolState::Failed,
                    Some("<tool_use_error>File does not exist.</tool_use_error>")
                )
            ),
            (5, Entry::Agent { text: "The README links a missing docs page; I fixed the link.".into() }),
        ]
    );
    assert_eq!(statuses(&first_turn), vec![SessionStatus::Waiting], "a finished turn is Needs you, never Paused");
    let facts = first_turn
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionFactsChanged { facts, .. } => Some(facts.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        facts,
        SessionFacts {
            model: Some("claude-opus-5-5".into()),
            cli_version: Some("2.1.300".into()),
            cli_permission_mode: Some("default".into()),
            turns: 1,
            ..SessionFacts::default()
        }
    );

    let follow_up = core.send(Command::SendPrompt { session_id: id.clone(), text: "Also check the CHANGELOG.".into() });
    assert_eq!(statuses(&follow_up), vec![SessionStatus::Running]);
    assert_eq!(entries(&follow_up), vec![(6, Entry::User { text: "Also check the CHANGELOG.".into() })]);
    assert_eq!(
        core.agents.commands(0).last(),
        Some(&AgentCommand::SendText { text: "Also check the CHANGELOG.".into() })
    );

    let rest = core.agent_replays(0, &recording[first_turn_end + 1..]);
    assert_eq!(statuses(&rest), vec![SessionStatus::Waiting, SessionStatus::Done]);
}

#[test]
fn stop_interrupts_the_turn_marks_the_session_paused_and_leaves_the_worktree_untouched() {
    let (mut core, _repo, worktree, _) = setup();
    // The agent has already changed things: an edit and a new file, uncommitted.
    std::fs::write(worktree.join("README"), "hello, edited\n").unwrap();
    std::fs::write(worktree.join("notes.txt"), "draft\n").unwrap();
    let before = harness::worktree_snapshot(&worktree);
    let id = started_session(&core.start_session(&worktree, "Run the tests."));
    let recording = recorded_events("claude-code/interrupted-turn");
    core.agent_replays(0, &recording[..3]); // init, prose, the shell run starts

    let stopped = core.send(Command::StopSession { session_id: id.clone() });

    assert_eq!(statuses(&stopped), vec![SessionStatus::Idle]);
    assert_eq!(entries(&stopped), vec![(4, Entry::System { text: "Stopped by you.".into() })]);
    assert_eq!(core.agents.commands(0).last(), Some(&AgentCommand::Interrupt));

    // The CLI winds the turn down after the interrupt; the session stays Paused.
    let after = core.agent_replays(0, &recording[3..]);
    assert_eq!(statuses(&after), vec![]);
    assert_eq!(harness::worktree_snapshot(&worktree), before);

    let again = core.send(Command::StopSession { session_id: id });
    assert!(
        matches!(&again[..], [Event::CommandRefused { command, .. }] if command == "stopSession"),
        "{again:#?}"
    );
}

#[test]
fn stop_also_works_while_the_session_needs_you() {
    let (mut core, _repo, worktree, _) = setup();
    let id = started_session(&core.start_session(&worktree, "Hi"));
    core.agent_says(0, AgentEvent::TurnComplete);

    let stopped = core.send(Command::StopSession { session_id: id });

    assert_eq!(statuses(&stopped), vec![SessionStatus::Idle]);
}

fn refusal(events: &[Event]) -> String {
    match events {
        [Event::CommandRefused { command, reason }] if command == "startSession" => reason.clone(),
        other => panic!("expected one startSession refusal, got {other:#?}"),
    }
}

#[test]
fn starting_on_a_worktree_whose_session_is_running_or_needs_you_is_refused() {
    let (mut core, _repo, worktree, _) = setup();
    core.start_session(&worktree, "First");

    let while_running = core.start_session(&worktree, "Second");
    assert!(refusal(&while_running).contains("Running"), "{while_running:#?}");

    core.agent_says(0, AgentEvent::TurnComplete);
    let while_waiting = core.start_session(&worktree, "Second");
    assert!(refusal(&while_waiting).contains("Needs you"), "{while_waiting:#?}");

    assert_eq!(core.agents.launches().len(), 1, "no second agent was launched");
}

#[test]
fn a_paused_finished_or_failed_occupant_still_blocks_a_second_session() {
    // One session per worktree (ADR-0005); "stop and replace" arrives with archiving.
    let (mut core, _repo, worktree, _) = setup();
    let id = started_session(&core.start_session(&worktree, "First"));
    core.send(Command::StopSession { session_id: id });

    let events = core.start_session(&worktree, "Second");

    assert!(refusal(&events).contains("Paused"), "{events:#?}");
    assert_eq!(core.agents.launches().len(), 1);
}

#[test]
fn only_explore_sessions_may_start_on_the_primary_worktree() {
    let (mut core, repo, _worktree, _) = setup();

    for mode in [PermissionMode::AskFirst, PermissionMode::AutoEdit, PermissionMode::FullAuto] {
        let events = core.start_session_in(&repo.path, mode, "Edit main");
        assert!(refusal(&events).contains("primary worktree"), "{mode:?}: {events:#?}");
    }
    assert!(core.agents.launches().is_empty());

    let explore = core.start_session_in(&repo.path, PermissionMode::Explore, "How does this work?");
    started_session(&explore);
}

#[test]
fn a_folder_that_is_not_a_known_worktree_is_refused() {
    let (mut core, _repo, _worktree, _) = setup();
    let elsewhere = TempRepo::new("other");

    let events = core.start_session(&elsewhere.path, "Hi");

    assert!(refusal(&events).contains("not a worktree of an added repository"), "{events:#?}");
}

#[test]
fn an_agent_that_cannot_be_launched_refuses_the_start_with_the_reason() {
    let (mut core, _repo, worktree, _) = setup();
    core.agents.refuse_next_launch("node was not found");

    let events = core.start_session(&worktree, "Hi");

    assert_eq!(refusal(&events), "Could not start Claude Code: node was not found");
    // Nothing occupies the worktree afterwards.
    started_session(&core.start_session(&worktree, "Hi again"));
}

#[test]
fn the_agent_process_exiting_mid_turn_is_an_error_with_its_last_words() {
    let (mut core, _repo, worktree, _) = setup();
    core.start_session(&worktree, "Hi");

    let events = core.agent_exits(0, Some(1), "Error: not logged in\n");

    assert_eq!(statuses(&events), vec![SessionStatus::Failed]);
    assert_eq!(
        entries(&events),
        vec![(1, Entry::System { text: "The agent process exited (code 1).\nError: not logged in".into() })]
    );
}

#[test]
fn a_fatal_agent_error_fails_the_session_and_a_recoverable_one_is_only_noted() {
    let (mut core, _repo, worktree, _) = setup();
    core.start_session(&worktree, "Hi");

    let recoverable = core.agent_says(0, AgentEvent::Error { message: "rate limited".into(), fatal: false });
    assert_eq!(statuses(&recoverable), vec![]);
    assert_eq!(entries(&recoverable), vec![(1, Entry::System { text: "Agent error: rate limited".into() })]);

    let fatal = core.agent_says(0, AgentEvent::Error { message: "query failed".into(), fatal: true });
    assert_eq!(statuses(&fatal), vec![SessionStatus::Failed]);
}

#[test]
fn a_paused_session_whose_process_exits_stays_paused() {
    let (mut core, _repo, worktree, _) = setup();
    let id = started_session(&core.start_session(&worktree, "Hi"));
    core.send(Command::StopSession { session_id: id });

    let events = core.agent_exits(0, Some(0), "");

    assert_eq!(statuses(&events), vec![]);
}

#[test]
fn attaching_replays_every_session_with_its_status_facts_and_transcript() {
    let (mut core, _repo, worktree, _) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix the broken link in the README."));
    let recording = recorded_events("claude-code/first-session");
    core.agent_replays(0, &recording[..9]); // through the first turn_complete

    let events = core.send(Command::Attach);

    let session_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(e, Event::SessionStarted { .. } | Event::SessionFactsChanged { .. } | Event::TranscriptEntry { .. })
        })
        .collect();
    match session_events[0] {
        Event::SessionStarted { session } => {
            assert_eq!(session.id, id);
            assert_eq!(session.status, SessionStatus::Waiting);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(session_events[1], Event::SessionFactsChanged { facts, .. } if facts.turns == 1));
    let transcript = entries(&events);
    assert_eq!(transcript.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(transcript[3].1, tool("toolu_01", "Bash", "ls", ToolState::Ok, Some("README.md\npackage.json\nsrc")));
}

#[test]
fn an_empty_prompt_is_refused() {
    let (mut core, _repo, worktree, _) = setup();

    let events = core.start_session(&worktree, "   ");

    assert!(refusal(&events).contains("prompt"), "{events:#?}");
}
