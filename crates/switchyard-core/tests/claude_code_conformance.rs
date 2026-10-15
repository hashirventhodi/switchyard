//! Seam 2 for the Claude Code adapter. The adapter lives in the Node sidecar
//! (`sidecar/`); this test runs the sidecar's own translator in replay mode
//! (`node sidecar/src/replay.ts`, no SDK, no CLI, no model) and drives it through
//! the conformance runner with the recorded fixtures under
//! `crates/adapter-protocol/fixtures/claude-code/`.
//!
//! Requires Node >= 22.18 on PATH (it runs the TypeScript source directly).

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command as Process, Stdio};

use adapter_protocol::conformance::{self, Fixture};
use adapter_protocol::{Adapter, AdapterError, AdapterManifest, Command, Event};
use serde_json::{json, Value};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The sidecar's translator behind its replay framing: one JSON request per
/// line in (`{"manifest":true}`, `{"ingest":raw}`, `{"command":cmd}`), one JSON
/// reply per line out.
struct ReplaySidecar {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ReplaySidecar {
    fn spawn() -> Self {
        let mut child = Process::new("node")
            .arg(workspace_root().join("sidecar/src/replay.ts"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("node is on PATH");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        ReplaySidecar { child, stdin, stdout }
    }

    fn ask(&mut self, request: Value) -> Result<Value, AdapterError> {
        writeln!(self.stdin, "{request}").map_err(|e| AdapterError(e.to_string()))?;
        let mut line = String::new();
        self.stdout.read_line(&mut line).map_err(|e| AdapterError(e.to_string()))?;
        let reply: Value = serde_json::from_str(&line)
            .map_err(|e| AdapterError(format!("bad reply {line:?}: {e}")))?;
        match reply.get("error") {
            Some(error) => Err(AdapterError(error.to_string())),
            None => Ok(reply),
        }
    }
}

impl Drop for ReplaySidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Adapter for ReplaySidecar {
    fn manifest(&self) -> AdapterManifest {
        // `manifest` takes &self; the replay process answers it on a fresh instance.
        let mut probe = ReplaySidecar::spawn();
        let reply = probe.ask(json!({ "manifest": true })).expect("manifest");
        serde_json::from_value(reply["manifest"].clone()).expect("manifest shape")
    }

    fn ingest(&mut self, raw: &str) -> Result<Vec<Event>, AdapterError> {
        let reply = self.ask(json!({ "ingest": raw }))?;
        serde_json::from_value(reply["events"].clone()).map_err(|e| AdapterError(e.to_string()))
    }

    fn command(&mut self, command: &Command) -> Result<Vec<String>, AdapterError> {
        let reply = self.ask(json!({ "command": command }))?;
        let responses: Vec<Value> = serde_json::from_value(reply["responses"].clone())
            .map_err(|e| AdapterError(e.to_string()))?;
        Ok(responses.iter().map(Value::to_string).collect())
    }
}

#[test]
fn claude_code_sidecar_passes_every_recorded_session() {
    let root = workspace_root().join("crates/adapter-protocol/fixtures/claude-code");
    let fixtures = Fixture::load_all(&root).expect("fixtures load");
    assert!(fixtures.len() >= 2, "{fixtures:?}");
    for fixture in fixtures {
        assert_eq!(fixture.meta.cli.version, "2.1.300", "{}", fixture.name);
        if let Err(failure) = conformance::run(&mut ReplaySidecar::spawn(), &fixture) {
            panic!("fixture {} failed:\n{failure}", fixture.name);
        }
    }
}
