//! The conformance runner: replays a recorded raw CLI transcript into an
//! adapter, diffs the events it emits against the expected list, and checks
//! each scripted command (verdicts included) is translated into the exact
//! CLI-side response. The fixture format is documented in README.md.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adapter::{Adapter, AdapterError};
use crate::protocol::{CliInfo, Command, Event, PROTOCOL_VERSION};

/// `meta.json` of a fixture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureMeta {
    /// Name of the adapter the recording is for.
    pub adapter: String,
    /// The CLI and the exact version the recording was made against.
    pub cli: CliInfo,
    /// The protocol version the expected events are written in.
    pub protocol: Version,
    #[serde(default)]
    pub description: Option<String>,
}

/// One line of `commands.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptedCommand {
    /// Send the command once this many lines of `input.jsonl` have been ingested.
    pub after_input: usize,
    pub command: Command,
    /// The raw messages the adapter must write to the CLI in response. A raw
    /// message that parses as JSON is compared as JSON; otherwise as a string.
    pub expect_responses: Vec<Value>,
}

/// A recorded session, loaded from a fixture directory.
#[derive(Debug, Clone, PartialEq)]
pub struct Fixture {
    /// The fixture directory's name.
    pub name: String,
    pub meta: FixtureMeta,
    /// Raw CLI output, one message per line, passed to the adapter verbatim.
    pub input: Vec<String>,
    pub expected_events: Vec<Event>,
    pub commands: Vec<ScriptedCommand>,
}

#[derive(Debug)]
pub struct FixtureError {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}: {}", self.path.display(), self.message),
            None => write!(f, "{}: {}", self.path.display(), self.message),
        }
    }
}

impl std::error::Error for FixtureError {}

impl Fixture {
    pub fn load(dir: impl AsRef<Path>) -> Result<Fixture, FixtureError> {
        let dir = dir.as_ref();
        let meta_path = dir.join("meta.json");
        let meta: FixtureMeta = serde_json::from_str(&read(&meta_path)?)
            .map_err(|e| error(&meta_path, None, e))?;
        let input_path = dir.join("input.jsonl");
        let input = lines(&read(&input_path)?).map(|(_, l)| l.to_owned()).collect();
        let expected_events = parse_jsonl(&dir.join("expected-events.jsonl"))?;
        let commands_path = dir.join("commands.jsonl");
        let commands: Vec<ScriptedCommand> = if commands_path.exists() {
            parse_jsonl(&commands_path)?
        } else {
            vec![]
        };
        if commands.windows(2).any(|w| w[0].after_input > w[1].after_input) {
            return Err(error(&commands_path, None, "commands must be ordered by after_input"));
        }
        Ok(Fixture {
            name: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            meta,
            input,
            expected_events,
            commands,
        })
    }

    /// Loads every fixture directory (one containing `meta.json`) directly
    /// under `root`, sorted by name.
    pub fn load_all(root: impl AsRef<Path>) -> Result<Vec<Fixture>, FixtureError> {
        let root = root.as_ref();
        let mut dirs: Vec<PathBuf> = fs::read_dir(root)
            .map_err(|e| error(root, None, e))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.join("meta.json").is_file())
            .collect();
        dirs.sort();
        dirs.iter().map(Fixture::load).collect()
    }
}

fn read(path: &Path) -> Result<String, FixtureError> {
    fs::read_to_string(path).map_err(|e| error(path, None, e))
}

fn error(path: &Path, line: Option<usize>, message: impl fmt::Display) -> FixtureError {
    FixtureError { path: path.to_owned(), line, message: message.to_string() }
}

/// Non-blank lines with their 1-based line numbers.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()).map(|(i, l)| (i + 1, l))
}

fn parse_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, FixtureError> {
    lines(&read(path)?)
        .map(|(n, l)| serde_json::from_str(l).map_err(|e| error(path, Some(n), e)))
        .collect()
}

/// Why a conformance run did not pass.
#[derive(Debug)]
pub enum ConformanceFailure {
    /// The adapter's declared range does not include [`PROTOCOL_VERSION`].
    ProtocolMismatch { adapter: String, implements: VersionReq, protocol: Version },
    /// The fixture's expected events are written in a protocol version this
    /// runner cannot read (semver-incompatible with, or newer than, its own).
    FixtureProtocolMismatch { fixture: Version, protocol: Version },
    /// The adapter returned an error for a raw input line or a command.
    Adapter { during: String, error: AdapterError },
    /// The adapter's output differed from the recording.
    Mismatch(Vec<Mismatch>),
}

#[derive(Debug)]
pub struct Mismatch {
    pub what: String,
    pub expected: Vec<String>,
    pub actual: Vec<String>,
}

impl Mismatch {
    /// A unified diff, one compact JSON value per line, with three lines of context.
    pub fn diff(&self) -> String {
        let expected = lines_with_newlines(&self.expected);
        let actual = lines_with_newlines(&self.actual);
        similar::TextDiff::from_lines(&expected, &actual)
            .unified_diff()
            .context_radius(3)
            .header("expected", "actual")
            .to_string()
    }
}

fn lines_with_newlines(lines: &[String]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

impl fmt::Display for ConformanceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConformanceFailure::ProtocolMismatch { adapter, implements, protocol } => write!(
                f,
                "adapter {adapter} implements protocol {implements}, which does not include protocol {protocol}"
            ),
            ConformanceFailure::FixtureProtocolMismatch { fixture, protocol } => write!(
                f,
                "fixture is recorded for protocol {fixture}, which is incompatible with protocol {protocol}"
            ),
            ConformanceFailure::Adapter { during, error } => {
                write!(f, "adapter error during {during}: {error}")
            }
            ConformanceFailure::Mismatch(mismatches) => {
                for m in mismatches {
                    writeln!(f, "{} differ:", m.what)?;
                    write!(f, "{}", m.diff())?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConformanceFailure {}

/// Runs one fixture against an adapter.
pub fn run(adapter: &mut dyn Adapter, fixture: &Fixture) -> Result<(), ConformanceFailure> {
    let manifest = adapter.manifest();
    if !manifest.implements.matches(&PROTOCOL_VERSION) {
        return Err(ConformanceFailure::ProtocolMismatch {
            adapter: manifest.name,
            implements: manifest.implements,
            protocol: PROTOCOL_VERSION,
        });
    }
    if !compatible(&fixture.meta.protocol) {
        return Err(ConformanceFailure::FixtureProtocolMismatch {
            fixture: fixture.meta.protocol.clone(),
            protocol: PROTOCOL_VERSION,
        });
    }

    let mut events = Vec::new();
    let mut mismatches = Vec::new();
    let mut pending = fixture.commands.iter().enumerate().peekable();

    for position in 0..=fixture.input.len() {
        while let Some((index, scripted)) = pending.next_if(|(_, c)| c.after_input <= position) {
            let during = format!("command #{} {}", index + 1, compact(&scripted.command));
            let responses = adapter
                .command(&scripted.command)
                .map_err(|error| ConformanceFailure::Adapter { during: during.clone(), error })?;
            let actual: Vec<Value> = responses.iter().map(|r| raw_to_value(r)).collect();
            if actual != scripted.expect_responses {
                mismatches.push(Mismatch {
                    what: format!("responses to {during}"),
                    expected: scripted.expect_responses.iter().map(compact).collect(),
                    actual: actual.iter().map(compact).collect(),
                });
            }
        }
        if let Some(raw) = fixture.input.get(position) {
            let emitted = adapter.ingest(raw).map_err(|error| ConformanceFailure::Adapter {
                during: format!("input line {}", position + 1),
                error,
            })?;
            events.extend(emitted);
        }
    }

    if events != fixture.expected_events {
        mismatches.insert(
            0,
            Mismatch {
                what: "events".into(),
                expected: fixture.expected_events.iter().map(compact).collect(),
                actual: events.iter().map(compact).collect(),
            },
        );
    }

    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(ConformanceFailure::Mismatch(mismatches))
    }
}

/// A fixture written in `version` is readable when `^version` includes ours.
fn compatible(version: &Version) -> bool {
    VersionReq::parse(&format!("^{version}")).is_ok_and(|req| req.matches(&PROTOCOL_VERSION))
}

fn raw_to_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

fn compact(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("protocol types serialize")
}
