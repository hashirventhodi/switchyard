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

#[test]
fn only_the_newest_256_kb_of_raw_events_are_kept_and_replayed_with_their_numbers() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    started_session(&core.start_session(&worktree, "hi"));
    let big = "x".repeat(10 * 1024);
    for _ in 0..60 {
        core.agent_says(0, adapter_protocol::Event::AgentText { text: big.clone() });
    }
    core.agent_says(0, adapter_protocol::Event::AgentText { text: "last".into() });

    let replayed = raw(&core.send(Command::Attach));
    let bytes: usize = replayed.iter().map(|(_, _, v)| v.to_string().len()).sum();
    assert!(bytes <= 256 * 1024, "replayed {bytes} bytes");
    assert!(replayed.len() >= 20, "keeps the newest, not nothing: {}", replayed.len());
    let numbers: Vec<u32> = replayed.iter().map(|(_, i, _)| *i).collect();
    assert_eq!(*numbers.last().unwrap(), 60, "events keep their numbers");
    assert!(numbers.windows(2).all(|w| w[1] == w[0] + 1), "the newest, contiguous: {numbers:?}");
    assert_eq!(replayed.last().unwrap().2["text"], "last");
}
