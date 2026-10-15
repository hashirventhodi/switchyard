use semver::VersionReq;
use serde::{Deserialize, Serialize};

use crate::protocol::{Command, Event};

/// What an adapter says about itself. `implements` is the semver range of
/// protocol versions it speaks; hosts and the conformance runner refuse an
/// adapter whose range does not match [`crate::PROTOCOL_VERSION`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterManifest {
    pub name: String,
    pub implements: VersionReq,
}

/// A translator between one agent CLI's structured output and the protocol.
///
/// The adapter never touches a process itself: the host feeds it the CLI's
/// raw output one message (line) at a time and writes whatever raw messages
/// it returns back to the CLI. That keeps adapters deterministic under
/// recorded fixtures, and lets an out-of-process adapter (a Node sidecar,
/// a bridge to `codex app-server`) implement this trait over its own
/// JSON-RPC transport.
pub trait Adapter {
    fn manifest(&self) -> AdapterManifest;

    /// One raw message from the CLI in; zero or more protocol events out.
    fn ingest(&mut self, raw: &str) -> Result<Vec<Event>, AdapterError>;

    /// One protocol command in; zero or more raw messages for the CLI out.
    fn command(&mut self, command: &Command) -> Result<Vec<String>, AdapterError>;

    /// Raw messages the adapter must write to the CLI on its own, in reply to what it
    /// ingested rather than to a host command: a JSON-RPC CLI's handshake and follow-up
    /// requests (start a thread once initialised, start a turn once the thread exists),
    /// or an error answer to a server request the adapter cannot handle. Hosts call it
    /// after every `ingest` and write what it returns, in order. Most adapters write
    /// nothing on their own.
    fn replies(&mut self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError(pub String);

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AdapterError {}
