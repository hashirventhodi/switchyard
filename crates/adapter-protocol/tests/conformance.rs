//! Seam 2: the conformance runner, driven from outside with recorded fixtures.

use std::path::PathBuf;

use adapter_protocol::conformance::{self, Fixture};
use adapter_protocol::echo::EchoAdapter;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn basic_session() -> Fixture {
    Fixture::load(fixtures_root().join("echo/basic-session")).expect("fixture loads")
}

#[test]
fn echo_adapter_passes_every_echo_fixture() {
    let fixtures = Fixture::load_all(fixtures_root().join("echo")).expect("fixtures load");
    assert!(!fixtures.is_empty());
    for fixture in fixtures {
        if let Err(failure) = conformance::run(&mut EchoAdapter::new(), &fixture) {
            panic!("fixture {} failed:\n{failure}", fixture.name);
        }
    }
}

#[test]
fn basic_session_fixture_exercises_the_whole_protocol() {
    let fixture = basic_session();
    let types: std::collections::BTreeSet<String> = fixture
        .expected_events
        .iter()
        .map(|e| serde_json::to_value(e).unwrap()["type"].as_str().unwrap().to_owned())
        .collect();
    for t in [
        "session_init",
        "agent_text",
        "tool_call_started",
        "tool_call_finished",
        "permission_request",
        "turn_complete",
        "done",
        "error",
        "token_usage",
        "classifier_denial",
    ] {
        assert!(types.contains(t), "basic-session has no {t} event");
    }
    let commands: std::collections::BTreeSet<String> = fixture
        .commands
        .iter()
        .map(|c| serde_json::to_value(&c.command).unwrap()["type"].as_str().unwrap().to_owned())
        .collect();
    for t in ["send_text", "answer_request", "interrupt", "resume"] {
        assert!(commands.contains(t), "basic-session sends no {t} command");
    }
}

#[test]
fn altered_expected_event_fails_with_a_readable_diff() {
    let mut fixture = basic_session();
    fixture.expected_events[1] =
        adapter_protocol::Event::AgentText { text: "Looking at the tests.".into() };

    let failure = conformance::run(&mut EchoAdapter::new(), &fixture)
        .expect_err("altered fixture must fail")
        .to_string();

    assert!(failure.contains("events differ"), "{failure}");
    assert!(failure.contains("--- expected"), "{failure}");
    assert!(failure.contains("+++ actual"), "{failure}");
    assert!(
        failure.contains(r#"-{"type":"agent_text","text":"Looking at the tests."}"#),
        "{failure}"
    );
    assert!(
        failure.contains(r#"+{"type":"agent_text","text":"Looking at the build."}"#),
        "{failure}"
    );
    // Unchanged neighbours appear as context; distant events are left out.
    assert!(failure.contains(r#" {"type":"session_init""#), "{failure}");
    assert!(!failure.contains(r#""type":"done""#), "{failure}");
}

#[test]
fn verdict_translated_to_the_wrong_cli_response_fails_naming_the_command() {
    let mut fixture = basic_session();
    let answer = fixture
        .commands
        .iter_mut()
        .find(|c| {
            matches!(&c.command, adapter_protocol::Command::AnswerRequest { request_id, .. } if request_id == "r2")
        })
        .unwrap();
    answer.expect_responses = vec![serde_json::json!({ "reply_to": "r2", "allow": false })];

    let failure = conformance::run(&mut EchoAdapter::new(), &fixture)
        .expect_err("altered response must fail")
        .to_string();

    assert!(failure.contains("responses to command #3"), "{failure}");
    assert!(failure.contains(r#""request_id":"r2""#), "{failure}");
    assert!(failure.contains(r#"-{"allow":false,"reply_to":"r2"}"#), "{failure}");
    assert!(failure.contains("+{\"echo\":"), "{failure}");
    assert!(!failure.contains("events differ"), "{failure}");
}
