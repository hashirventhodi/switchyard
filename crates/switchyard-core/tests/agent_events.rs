//! The Agent log's debug view: every adapter-protocol event a session's agent sends is
//! passed on as is, numbered, and replayed on attach.

mod harness;

use harness::{recorded_events, started_session, Harness, TempRepo};
use switchyard_schema::{Command, Event};

fn raw(events: &[Event]) -> Vec<(String, u32, serde_json::Value)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::AgentEventReceived { session_id, index, json } => {
                Some((session_id.clone(), *index, serde_json::from_str(json).expect("valid JSON")))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn every_protocol_event_is_passed_on_raw_in_order_and_replayed_on_attach() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    let id = started_session(&core.start_session(&worktree, "hi"));
    let recorded = recorded_events("claude-code/first-session");

    let events = core.agent_replays(0, &recorded);

    let expected: Vec<_> = recorded
        .iter()
        .enumerate()
        .map(|(i, e)| (id.clone(), i as u32, serde_json::to_value(e).unwrap()))
        .collect();
    assert_eq!(raw(&events), expected);
    assert_eq!(expected[0].2["type"], "session_init", "the fixture starts with the CLI's init");

    let replayed = core.send(Command::Attach);
    assert_eq!(raw(&replayed), expected);
}
