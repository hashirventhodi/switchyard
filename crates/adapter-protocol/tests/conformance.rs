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
