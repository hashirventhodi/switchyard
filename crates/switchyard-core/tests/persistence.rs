//! Sessions outlive the app: transcripts, facts and spend are stored; a relaunch kills
//! orphaned agent processes (by pid and start time, never adopting them), cancels pending
//! Permission requests and brings Running / Needs you sessions back Paused. Nothing
//! auto-resumes; Resume relaunches the agent through its CLI's resume-by-id.

mod harness;

use std::path::{Path, PathBuf};

use adapter_protocol::{CliInfo, Command as AgentCommand, Event as AgentEvent, PermissionMode as ProtocolMode, RequestDetail};
use harness::{added_repository, started_session, Harness, TempRepo, NOW_MS};
use switchyard_schema::{AgentKind, Command, Entry, Event, PermissionMode, RequestState, SessionStatus, Settings, Verdict};

fn setup() -> (Harness, TempRepo, PathBuf) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    (core, repo, worktree)
}

fn init(external: &str) -> AgentEvent {
    AgentEvent::SessionInit {
        external_session_id: external.into(),
        permission_mode: ProtocolMode::AskFirst,
        cli_permission_mode: "default".into(),
        cli: CliInfo { name: "claude-code".into(), version: "2.1.300".into() },
        model: Some("claude-opus-5-5".into()),
    }
}

fn text(t: &str) -> AgentEvent {
    AgentEvent::AgentText { text: t.into() }
}

fn command_request(id: &str) -> AgentEvent {
    AgentEvent::PermissionRequest {
        request_id: id.into(),
        request: RequestDetail::Command { command: "pnpm test".into(), cwd: None },
        reason: None,
    }
}

/// The status a session is replayed with on attach, if it is replayed as a live session.
fn replayed_status(events: &[Event], id: &str) -> Option<SessionStatus> {
    events.iter().find_map(|e| match e {
        Event::SessionStarted { session } if session.id == id => Some(session.status),
        _ => None,
    })
}

/// A session's transcript as the renderer would build it from `events`.
fn transcript(events: &[Event], id: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for e in events {
        if let Event::TranscriptEntry { session_id, index, entry } = e {
            if session_id == id {
                let i = *index as usize;
                if i == out.len() {
                    out.push(entry.clone());
                } else {
                    out[i] = entry.clone();
                }
            }
        }
    }
    out
}

fn refusal(events: &[Event]) -> String {
    events
        .iter()
        .find_map(|e| match e {
            Event::CommandRefused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a refusal in {events:#?}"))
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

fn archived(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::SessionArchived { archived } => Some(archived.session.id.clone()),
            _ => None,
        })
        .collect()
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn a_session_comes_back_after_relaunch_with_its_transcript_facts_and_spend() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix the README."));
    core.agent_says(0, init("ext-1"));
    core.agent_says(0, text("Fixed the link."));
    core.agent_says(
        0,
        AgentEvent::TokenUsage { input_tokens: 10, output_tokens: 20, cache_read_tokens: 30, cost_usd: Some(0.5) },
    );
    core.agent_says(0, AgentEvent::TurnComplete);
    core.agent_says(0, AgentEvent::Done { summary: None });
    let before = transcript(&core.send(Command::Attach), &id);

    let mut core = core.relaunch();
    let events = core.send(Command::Attach);

    assert_eq!(replayed_status(&events, &id), Some(SessionStatus::Done));
    assert_eq!(transcript(&events, &id), before);
    let facts = events
        .iter()
        .find_map(|e| match e {
            Event::SessionFactsChanged { session_id, facts } if *session_id == id => Some(facts.clone()),
            _ => None,
        })
        .expect("facts replayed");
    assert_eq!(facts.turns, 1);
    assert_eq!(facts.model.as_deref(), Some("claude-opus-5-5"));
    let spend: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::SpendRecorded { session_id, spend, at_ms } if *session_id == id => Some((*at_ms, spend.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(spend.len(), 1);
    assert_eq!(spend[0].0, NOW_MS);
    assert_eq!((spend[0].1.input_tokens, spend[0].1.output_tokens, spend[0].1.cache_read_tokens), (10, 20, 30));
    assert_eq!(spend[0].1.reported_usd, Some(0.5));
}

#[test]
fn entries_changed_long_after_they_were_written_come_back_changed() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Test it"));
    core.agent_says(0, AgentEvent::ToolCallStarted { call_id: "t1".into(), tool: "Bash".into(), input: serde_json::json!({"command": "ls"}) });
    core.agent_says(0, command_request("req-1"));
    for i in 0..200 {
        core.agent_says(0, text(&format!("line {i}")));
    }
    core.agent_says(0, AgentEvent::ToolCallFinished { call_id: "t1".into(), ok: true, output: Some("README".into()) });
    core.send(Command::AnswerRequest { session_id: id.clone(), request_id: "req-1".into(), verdict: Verdict::AllowOnce });
    let before = transcript(&core.send(Command::Attach), &id);

    let mut core = core.relaunch();
    let after = transcript(&core.send(Command::Attach), &id);

    assert_eq!(after[..before.len()], before[..]);
    assert!(matches!(&after[2], Entry::Request { state: RequestState::Allowed, .. }), "{:?}", after[2]);
}

#[test]
fn new_sessions_after_relaunch_get_fresh_ids() {
    let (mut core, repo, worktree) = setup();
    let first = started_session(&core.start_session(&worktree, "One"));
    let other = repo.add_worktree("feat/two");

    let mut core = core.relaunch();
    let second = started_session(&core.start_session(&other, "Two"));

    assert_ne!(first, second);
}

#[test]
fn running_and_waiting_sessions_come_back_paused_and_nothing_resumes() {
    let (mut core, repo, worktree) = setup();
    let running = started_session(&core.start_session(&worktree, "Keep going"));
    let other = repo.add_worktree("feat/two");
    let waiting = started_session(&core.start_session(&other, "Ask me"));
    core.agent_says(1, AgentEvent::TurnComplete);

    let mut core = core.relaunch();
    let events = core.send(Command::Attach);

    assert_eq!(replayed_status(&events, &running), Some(SessionStatus::Idle));
    assert_eq!(replayed_status(&events, &waiting), Some(SessionStatus::Idle));
    let last = transcript(&events, &running).pop().unwrap();
    assert!(matches!(&last, Entry::System { text } if text.contains("Paused") && text.contains("Resume")), "{last:?}");
    assert!(core.agents.launches().is_empty(), "nothing auto-resumes");
}

#[test]
fn on_relaunch_an_orphan_with_its_recorded_start_time_is_killed_and_a_reused_pid_is_left_alone() {
    let (mut core, repo, worktree) = setup();
    core.start_session(&worktree, "One"); // pid 4001
    let other = repo.add_worktree("feat/two");
    core.start_session(&other, "Two"); // pid 4002
    core.processes.reuse_pid(4002);

    let core = core.relaunch();

    assert_eq!(core.processes.killed(), vec![4001]);
    assert!(core.processes.is_running(4002), "a different program now owns pid 4002");
}

#[test]
fn an_agent_that_exited_is_not_killed_on_relaunch() {
    let (mut core, _repo, worktree) = setup();
    core.start_session(&worktree, "One");
    core.agent_exits(0, Some(0), "");
    core.processes.end(4001);

    let core = core.relaunch();

    assert!(core.processes.killed().is_empty());
}

#[test]
fn a_request_pending_at_the_crash_is_cancelled_with_a_system_line_and_cannot_be_answered() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Test it"));
    core.agent_says(0, command_request("req-1"));

    let mut core = core.relaunch();
    let events = core.send(Command::Attach);

    let entries = transcript(&events, &id);
    let card = entries.iter().find(|e| matches!(e, Entry::Request { .. })).unwrap();
    assert!(matches!(card, Entry::Request { state: RequestState::Cancelled, .. }), "{card:?}");
    assert!(
        entries.iter().any(|e| matches!(e, Entry::System { text } if text.contains("cancelled"))),
        "{entries:#?}"
    );
    let answer = core.send(Command::AnswerRequest {
        session_id: id.clone(),
        request_id: "req-1".into(),
        verdict: Verdict::AllowOnce,
    });
    refusal(&answer);
    assert!(core.agents.launches().is_empty());
}

#[test]
fn resume_after_relaunch_relaunches_the_agent_by_its_external_session_id() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix it"));
    core.agent_says(0, init("ext-42"));

    let mut core = core.relaunch();
    let events = core.send(Command::ResumeSession { session_id: id.clone() });

    assert_eq!(statuses(&events), vec![SessionStatus::Running]);
    let launches = core.agents.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].cwd, worktree);
    assert_eq!(launches[0].permission_mode, PermissionMode::AskFirst);
    let commands = core.agents.commands(0);
    assert_eq!(commands[0], AgentCommand::Resume { external_session_id: "ext-42".into() });
    assert!(matches!(&commands[1], AgentCommand::SendText { .. }), "{commands:?}");
    // The relaunched agent is recorded like any other: the next crash kills it too.
    let core = core.relaunch();
    assert_eq!(core.processes.killed(), vec![4001, 4002]);
}

#[test]
fn resuming_a_paused_session_whose_agent_still_runs_continues_it_without_a_relaunch() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix it"));
    core.agent_says(0, init("ext-1"));
    core.send(Command::StopSession { session_id: id.clone() });

    let events = core.send(Command::ResumeSession { session_id: id });

    assert_eq!(statuses(&events), vec![SessionStatus::Running]);
    assert_eq!(core.agents.launches().len(), 1);
    assert!(matches!(core.agents.commands(0).last(), Some(AgentCommand::SendText { .. })));
}

#[test]
fn a_running_session_cannot_be_resumed() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix it"));
    core.agent_says(0, init("ext-1"));

    let events = core.send(Command::ResumeSession { session_id: id });

    refusal(&events);
}

#[test]
fn a_session_whose_agent_never_started_cannot_be_resumed() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix it"));

    let mut core = core.relaunch();
    let events = core.send(Command::ResumeSession { session_id: id });

    assert!(refusal(&events).contains("resume"), "{events:#?}");
    assert!(core.agents.launches().is_empty());
}

#[test]
fn a_prompt_to_a_finished_session_after_relaunch_resumes_the_agent_first() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix it"));
    core.agent_says(0, init("ext-7"));
    core.agent_says(0, AgentEvent::TurnComplete);
    core.agent_says(0, AgentEvent::Done { summary: None });

    let mut core = core.relaunch();
    core.send(Command::SendPrompt { session_id: id, text: "One more thing".into() });

    assert_eq!(
        core.agents.commands(0),
        vec![
            AgentCommand::Resume { external_session_id: "ext-7".into() },
            AgentCommand::SendText { text: "One more thing".into() },
        ]
    );
}

// ---- stop and replace, archive ----

fn replace(core: &mut Harness, id: &str, prompt: &str) -> Vec<Event> {
    core.send(Command::ReplaceSession {
        session_id: id.into(),
        agent: AgentKind::ClaudeCode,
        permission_mode: PermissionMode::AskFirst,
        prompt: prompt.into(),
    })
}

#[test]
fn starting_on_a_paused_session_offers_stop_and_replace() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "One"));
    core.send(Command::StopSession { session_id: id });

    let events = core.start_session(&worktree, "Two");

    assert!(refusal(&events).contains("stop and replace"), "{events:#?}");
}

#[test]
fn replacing_a_paused_session_archives_it_and_starts_a_new_one_with_an_empty_transcript() {
    let (mut core, _repo, worktree) = setup();
    let old = started_session(&core.start_session(&worktree, "One"));
    core.agent_says(0, text("Working on one."));
    core.send(Command::StopSession { session_id: old.clone() });
    let old_transcript = transcript(&core.send(Command::Attach), &old);

    let events = replace(&mut core, &old, "Two");

    assert_eq!(archived(&events), vec![old.clone()]);
    let new = started_session(&events);
    assert_ne!(new, old);
    assert_eq!(transcript(&events, &new), vec![Entry::User { text: "Two".into() }]);
    assert_eq!(core.processes.killed(), vec![4001], "the replaced agent is stopped");
    let read = core.send(Command::ReadArchivedSession { session_id: old.clone() });
    let entries = read.iter().find_map(|e| match e {
        Event::ArchivedTranscript { session_id, entries } if *session_id == old => Some(entries.clone()),
        _ => None,
    });
    assert_eq!(entries, Some(old_transcript));
}

#[test]
fn a_running_session_is_never_replaced() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "One"));

    let events = replace(&mut core, &id, "Two");

    refusal(&events);
    assert!(archived(&events).is_empty());
    assert_eq!(core.agents.launches().len(), 1);
}

#[test]
fn a_replace_that_cannot_start_keeps_the_old_session() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "One"));
    core.send(Command::StopSession { session_id: id.clone() });
    core.agents.refuse_next_launch("boom");

    let events = replace(&mut core, &id, "Two");

    refusal(&events);
    assert!(archived(&events).is_empty());
    let attach = core.send(Command::Attach);
    assert_eq!(replayed_status(&attach, &id), Some(SessionStatus::Idle));
}

#[test]
fn archived_sessions_are_listed_on_attach_after_relaunch_and_not_as_live_sessions() {
    let (mut core, _repo, worktree) = setup();
    let old = started_session(&core.start_session(&worktree, "One"));
    core.send(Command::StopSession { session_id: old.clone() });
    replace(&mut core, &old, "Two");

    let mut core = core.relaunch();
    let events = core.send(Command::Attach);

    assert_eq!(archived(&events), vec![old.clone()]);
    assert_eq!(replayed_status(&events, &old), None);
    let listed = events
        .iter()
        .find_map(|e| match e {
            Event::SessionArchived { archived } => Some(archived.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(listed.session.branch.as_deref(), Some("fix/readme"));
    assert_eq!(listed.archived_at_ms, NOW_MS);
}

#[test]
fn removing_a_worktree_with_a_paused_session_archives_it_and_the_archive_outlives_the_worktree() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "One"));
    core.agent_says(0, text("Did a thing."));
    core.send(Command::StopSession { session_id: id.clone() });

    let events = core.send(Command::RemoveWorktree { worktree_path: path(&worktree) });

    assert_eq!(archived(&events), vec![id.clone()]);
    assert!(events.iter().any(|e| matches!(e, Event::WorktreeRemoved { .. })), "{events:#?}");
    assert!(!worktree.exists());
    let mut core = core.relaunch();
    let read = core.send(Command::ReadArchivedSession { session_id: id.clone() });
    assert!(
        read.iter().any(|e| matches!(e, Event::ArchivedTranscript { entries, .. } if entries.len() >= 2)),
        "{read:#?}"
    );
}

#[test]
fn a_worktree_with_a_running_session_is_still_not_removed() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "One"));

    let events = core.send(Command::RemoveWorktree { worktree_path: path(&worktree) });

    refusal(&events);
    assert!(archived(&events).is_empty());
    assert!(worktree.exists());
}

#[test]
fn a_dirty_worktree_is_not_removed_and_its_session_not_archived() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "One"));
    core.send(Command::StopSession { session_id: id });
    std::fs::write(worktree.join("new.txt"), "x").unwrap();

    let events = core.send(Command::RemoveWorktree { worktree_path: path(&worktree) });

    refusal(&events);
    assert!(archived(&events).is_empty());
}

// ---- retention ----

fn settings(events: &[Event]) -> Option<Settings> {
    events.iter().find_map(|e| match e {
        Event::SettingsChanged { settings } => Some(settings.clone()),
        _ => None,
    })
}

/// Archive `n` sessions on the worktree, one after another; returns their ids, oldest first.
fn archive_several(core: &mut Harness, worktree: &Path, n: usize) -> Vec<String> {
    let mut ids = vec![started_session(&core.start_session(worktree, "first"))];
    for i in 0..n {
        let last = ids.last().unwrap().clone();
        core.send(Command::StopSession { session_id: last.clone() });
        ids.push(started_session(&replace(core, &last, &format!("next {i}"))));
    }
    ids.pop();
    ids
}

fn pruned(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ArchivedSessionPruned { session_id, .. } => Some(session_id.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn retention_defaults_to_fifty_per_repository_and_thirty_days() {
    let (mut core, _repo, _) = setup();

    let s = settings(&core.send(Command::Attach)).expect("settings on attach");

    assert_eq!((s.archive_keep_per_repository, s.archive_keep_days), (50, 30));
}

#[test]
fn pruning_keeps_only_the_newest_archived_sessions_per_repository() {
    let (mut core, _repo, worktree) = setup();
    let changed = core.send(Command::UpdateSettings {
        settings: Settings { archive_keep_per_repository: 2, archive_keep_days: 30 },
    });
    assert_eq!(settings(&changed).map(|s| s.archive_keep_per_repository), Some(2));

    let ids = archive_several(&mut core, &worktree, 3);

    let mut core = core.relaunch();
    let events = core.send(Command::Attach);
    assert_eq!(archived(&events), vec![ids[1].clone(), ids[2].clone()]);
    let read = core.send(Command::ReadArchivedSession { session_id: ids[0].clone() });
    refusal(&read);
}

#[test]
fn lowering_the_count_prunes_at_once() {
    let (mut core, _repo, worktree) = setup();
    let ids = archive_several(&mut core, &worktree, 3);

    let events = core.send(Command::UpdateSettings {
        settings: Settings { archive_keep_per_repository: 1, archive_keep_days: 30 },
    });

    assert_eq!(pruned(&events), vec![ids[0].clone(), ids[1].clone()]);
}

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

#[test]
fn archived_sessions_older_than_the_age_setting_are_pruned_on_relaunch() {
    let (mut core, _repo, worktree) = setup();
    archive_several(&mut core, &worktree, 2);

    let core = core.relaunch_at(|| NOW_MS + 30 * DAY_MS);
    let mut core = core;
    assert_eq!(archived(&core.send(Command::Attach)).len(), 2, "exactly 30 days old is kept");

    let mut core = core.relaunch_at(|| NOW_MS + 31 * DAY_MS);
    assert!(archived(&core.send(Command::Attach)).is_empty());
}

#[test]
fn a_longer_age_setting_keeps_old_archives() {
    let (mut core, _repo, worktree) = setup();
    core.send(Command::UpdateSettings { settings: Settings { archive_keep_per_repository: 50, archive_keep_days: 90 } });
    archive_several(&mut core, &worktree, 1);

    let mut core = core.relaunch_at(|| NOW_MS + 60 * DAY_MS);

    assert_eq!(archived(&core.send(Command::Attach)).len(), 1);
}

// ---- export ----

#[test]
fn export_writes_one_json_object_per_transcript_entry() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix the README."));
    core.agent_says(0, text("On it."));
    core.agent_says(0, command_request("req-1"));
    core.send(Command::AnswerRequest { session_id: id.clone(), request_id: "req-1".into(), verdict: Verdict::Deny });
    let expected = transcript(&core.send(Command::Attach), &id);
    assert_eq!(expected.len(), 3);

    let events = core.send(Command::ExportTranscript { session_id: id.clone() });

    let file = events
        .iter()
        .find_map(|e| match e {
            Event::TranscriptExported { session_id, path } if *session_id == id => Some(path.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{events:#?}"));
    assert!(PathBuf::from(&file).starts_with(core.export_dir()));
    assert!(file.ends_with(".jsonl"));
    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<Entry> =
        text.lines().map(|l| serde_json::from_str(l).expect("each line is one JSON object")).collect();
    assert_eq!(lines, expected);
}

#[test]
fn an_archived_session_can_be_exported() {
    let (mut core, _repo, worktree) = setup();
    let old = started_session(&core.start_session(&worktree, "One"));
    core.send(Command::StopSession { session_id: old.clone() });
    replace(&mut core, &old, "Two");

    let events = core.send(Command::ExportTranscript { session_id: old.clone() });

    assert!(events.iter().any(|e| matches!(e, Event::TranscriptExported { .. })), "{events:#?}");
}

// ---- scale ----

#[test]
fn a_twenty_thousand_entry_session_reloads_in_under_a_second() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Go"));
    for i in 0..20_000 {
        core.agent_says(0, text(&format!("line {i} of a long, long session with plenty of words in it")));
    }
    let core = core.relaunch();

    let started = std::time::Instant::now();
    let mut core = core.relaunch();
    let events = core.send(Command::Attach);
    let took = started.elapsed();

    assert_eq!(transcript(&events, &id).len(), 20_002); // prompt, 20k lines, the pause note
    assert!(took < std::time::Duration::from_secs(1), "reload took {took:?}");
}
