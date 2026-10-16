//! A fake adapter for an imaginary "echo" CLI whose raw output is protocol
//! events wrapped as `{"echo": <event>}` and whose raw input is commands
//! wrapped the same way. Any other raw message (such as `{"heartbeat":true}`)
//! produces no events. It exists to prove the conformance runner before a
//! real adapter does, and as the smallest example of an [`Adapter`].

use semver::VersionReq;
use serde_json::{json, Value};

use crate::adapter::{Adapter, AdapterError, AdapterManifest};
use crate::protocol::{Command, Event};

#[derive(Debug, Default)]
pub struct EchoAdapter;

impl EchoAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Adapter for EchoAdapter {
    fn manifest(&self) -> AdapterManifest {
        AdapterManifest {
            name: "echo".into(),
            implements: VersionReq::parse("^0.2").expect("valid range"),
        }
    }

    fn ingest(&mut self, raw: &str) -> Result<Vec<Event>, AdapterError> {
        let value: Value =
            serde_json::from_str(raw).map_err(|e| AdapterError(format!("not JSON: {e}")))?;
        match value.get("echo") {
            Some(event) => serde_json::from_value(event.clone())
                .map(|event| vec![event])
                .map_err(|e| AdapterError(format!("bad echoed event: {e}"))),
            None => Ok(vec![]),
        }
    }

    fn command(&mut self, command: &Command) -> Result<Vec<String>, AdapterError> {
        Ok(vec![json!({ "echo": command }).to_string()])
    }
}
