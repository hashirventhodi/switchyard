//! Seam 2 for the Codex adapter: every recorded `codex app-server` session under
//! `crates/adapter-protocol/fixtures/codex/` replayed through the conformance runner. Each
//! fixture's `launch.json` is what the host started the adapter with (worktree, permission
//! mode, per-spawn config, MCP rules from the resolved Policy).

use std::path::PathBuf;

use adapter_protocol::conformance::{self, Fixture};
use codex_adapter::{CodexAdapter, Launch};

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../adapter-protocol/fixtures/codex")
}

fn launch(fixture: &Fixture) -> Launch {
    let path = fixtures_root().join(&fixture.name).join("launch.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn codex_adapter_passes_every_recorded_session() {
    let fixtures = Fixture::load_all(fixtures_root()).expect("fixtures load");
    assert!(fixtures.len() >= 6, "{fixtures:?}");
    for fixture in fixtures {
        assert_eq!(fixture.meta.cli.version, "0.144.1", "{}", fixture.name);
        let mut adapter = CodexAdapter::new(launch(&fixture));
        if let Err(failure) = conformance::run(&mut adapter, &fixture) {
            panic!("fixture {} failed:\n{failure}", fixture.name);
        }
    }
}

#[test]
fn the_recordings_cover_start_request_allow_deny_question_and_finish() {
    let fixtures = Fixture::load_all(fixtures_root()).expect("fixtures load");
    let all = |f: &dyn Fn(&serde_json::Value) -> bool| {
        fixtures
            .iter()
            .flat_map(|x| x.expected_events.iter())
            .map(|e| serde_json::to_value(e).unwrap())
            .any(|v| f(&v))
    };
    let verdicts: Vec<serde_json::Value> = fixtures
        .iter()
        .flat_map(|x| x.commands.iter())
        .map(|c| serde_json::to_value(&c.command).unwrap())
        .filter(|c| c["type"] == "answer_request")
        .map(|c| c["verdict"]["decision"].clone())
        .collect();
    assert!(all(&|v| v["type"] == "session_init"), "start");
    assert!(
        all(&|v| v["type"] == "permission_request" && v["request"]["kind"] == "command"),
        "request"
    );
    assert!(
        all(&|v| v["type"] == "permission_request" && v["request"]["kind"] == "question"),
        "question"
    );
    assert!(verdicts.contains(&"allow_once".into()), "allow");
    assert!(verdicts.contains(&"deny".into()), "deny");
    assert!(verdicts.contains(&"answer".into()), "answer");
    assert!(all(&|v| v["type"] == "turn_complete"), "finish");
}

#[test]
fn the_adapter_implements_this_protocol() {
    let manifest = codex_adapter::manifest();
    assert_eq!(manifest.name, "codex");
    assert!(manifest
        .implements
        .matches(&adapter_protocol::PROTOCOL_VERSION));
}
