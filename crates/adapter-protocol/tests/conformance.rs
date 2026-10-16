//! Seam 2: the conformance runner, driven from outside with recorded fixtures.

use std::path::PathBuf;

use adapter_protocol::conformance::{self, ConformanceFailure, Fixture};
use adapter_protocol::echo::EchoAdapter;
use adapter_protocol::{Adapter, AdapterError, AdapterManifest, Command, Event};
use semver::{Version, VersionReq};

/// The echo adapter, claiming whatever protocol range it is told to.
struct Claiming(&'static str);

impl Adapter for Claiming {
    fn manifest(&self) -> AdapterManifest {
        AdapterManifest {
            name: "claiming".into(),
            implements: VersionReq::parse(self.0).unwrap(),
        }
    }
    fn ingest(&mut self, raw: &str) -> Result<Vec<Event>, AdapterError> {
        EchoAdapter::new().ingest(raw)
    }
    fn command(&mut self, command: &Command) -> Result<Vec<String>, AdapterError> {
        EchoAdapter::new().command(command)
    }
}

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
        .map(|e| {
            serde_json::to_value(e).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
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
        .map(|c| {
            serde_json::to_value(&c.command).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    for t in ["send_text", "answer_request", "interrupt", "resume"] {
        assert!(commands.contains(t), "basic-session sends no {t} command");
    }
}

#[test]
fn altered_expected_event_fails_with_a_readable_diff() {
    let mut fixture = basic_session();
    fixture.expected_events[1] = adapter_protocol::Event::AgentText {
        text: "Looking at the tests.".into(),
    };

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
    assert!(
        failure.contains(r#"-{"allow":false,"reply_to":"r2"}"#),
        "{failure}"
    );
    assert!(failure.contains("+{\"echo\":"), "{failure}");
    assert!(!failure.contains("events differ"), "{failure}");
}

#[test]
fn command_scheduled_past_the_end_of_the_input_fails_instead_of_being_skipped() {
    let mut fixture = basic_session();
    fixture.commands.last_mut().unwrap().after_input = 99;
    let failure = conformance::run(&mut EchoAdapter::new(), &fixture)
        .expect_err("unreachable command must fail")
        .to_string();
    assert!(failure.contains("command #8"), "{failure}");
    assert!(failure.contains("99"), "{failure}");
}

#[test]
fn adapter_declaring_a_range_that_includes_the_protocol_is_accepted() {
    for range in ["^0.2", ">=0.2.0, <0.3.0", "=0.2.0"] {
        conformance::run(&mut Claiming(range), &basic_session())
            .unwrap_or_else(|f| panic!("{range} should be accepted: {f}"));
    }
}

#[test]
fn adapter_declaring_a_range_that_excludes_the_protocol_is_rejected() {
    for range in ["^0.1", "^0.3", "^1", "=0.0.9"] {
        let failure = conformance::run(&mut Claiming(range), &basic_session())
            .expect_err("mismatched range must be rejected");
        assert!(
            matches!(failure, ConformanceFailure::ProtocolMismatch { .. }),
            "{range}: {failure:?}"
        );
        let message = failure.to_string();
        assert!(message.contains("claiming"), "{message}");
        assert!(
            message.contains(range.trim_start_matches(['^', '='])),
            "{message}"
        );
        assert!(message.contains("0.2.0"), "{message}");
    }
}

#[test]
fn fixture_recorded_against_an_incompatible_protocol_is_rejected() {
    let mut fixture = basic_session();
    fixture.meta.protocol = Version::new(0, 3, 0);
    let failure = conformance::run(&mut EchoAdapter::new(), &fixture)
        .expect_err("incompatible fixture must be rejected");
    assert!(
        matches!(failure, ConformanceFailure::FixtureProtocolMismatch { .. }),
        "{failure:?}"
    );
    assert!(failure.to_string().contains("0.3.0"), "{failure}");
}

/// An adapter for a JSON-RPC-like CLI: it answers each `{"ping": n}` the CLI sends with
/// `{"pong": n}` on its own (no host command involved), and reports nothing.
struct Ponging {
    pending: Vec<String>,
}

impl Adapter for Ponging {
    fn manifest(&self) -> AdapterManifest {
        AdapterManifest {
            name: "ponging".into(),
            implements: VersionReq::parse("^0.2").unwrap(),
        }
    }
    fn ingest(&mut self, raw: &str) -> Result<Vec<Event>, AdapterError> {
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        if let Some(n) = value.get("ping") {
            self.pending
                .push(serde_json::json!({ "pong": n }).to_string());
        }
        Ok(vec![])
    }
    fn command(&mut self, _: &Command) -> Result<Vec<String>, AdapterError> {
        Ok(vec![])
    }
    fn replies(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending)
    }
}

fn ponging() -> Fixture {
    Fixture::load(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ponging"))
        .expect("fixture loads")
}

#[test]
fn replies_an_adapter_writes_on_its_own_are_checked_against_writes_jsonl() {
    let fixture = ponging();
    assert_eq!(fixture.writes.len(), 2);

    conformance::run(&mut Ponging { pending: vec![] }, &fixture).unwrap_or_else(|f| panic!("{f}"));
}

#[test]
fn a_reply_written_after_the_wrong_input_line_fails_naming_the_writes() {
    let mut fixture = ponging();
    fixture.writes[1].after_input = 2;

    let failure = conformance::run(&mut Ponging { pending: vec![] }, &fixture)
        .expect_err("misplaced reply must fail")
        .to_string();

    assert!(failure.contains("writes differ"), "{failure}");
    assert!(
        failure.contains(r#"-{"after_input":2,"message":{"pong":2}}"#),
        "{failure}"
    );
    assert!(
        failure.contains(r#"+{"after_input":3,"message":{"pong":2}}"#),
        "{failure}"
    );
}

#[test]
fn a_fixture_without_writes_requires_the_adapter_to_write_nothing_on_its_own() {
    let mut fixture = ponging();
    fixture.writes.clear();

    let failure = conformance::run(&mut Ponging { pending: vec![] }, &fixture)
        .expect_err("unexpected replies must fail")
        .to_string();

    assert!(failure.contains("writes differ"), "{failure}");
}
