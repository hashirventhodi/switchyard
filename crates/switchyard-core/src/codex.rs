//! Runs OpenAI Codex as `codex app-server` (JSON-RPC over stdio) in the worktree, with the
//! Codex adapter (`crates/codex-adapter`) translating in-process: every stdout line goes
//! through [`Adapter::ingest`] (events to the core, the adapter's own replies back to the
//! CLI), every protocol command through [`Adapter::command`]. Diagnostics on stderr are
//! kept for the transcript when the process exits.
//!
//! Per-spawn configuration reaches Codex only as the thread's `config` overrides (dotted
//! config.toml keys), never as writes to `~/.codex`: [`spawn_config`] is the one place to
//! add them.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use adapter_protocol::Adapter;
use codex_adapter::{CodexAdapter, Launch, McpRule};
use serde_json::{Map, Value};

use crate::mcp;
use crate::agents::{protocol_mode, AgentHandle, AgentLauncher, AgentOutput, AgentSink, LaunchSpec};

/// Lines of stderr kept for the transcript when the process exits.
const STDERR_TAIL: usize = 20;

pub struct CodexLauncher {
    /// The `codex` executable.
    pub codex: PathBuf,
    /// When set, every raw line Codex writes and every line written to it is appended to
    /// `<dir>/codex-<pid>.{input,writes}.jsonl`, for recording conformance fixtures.
    pub record_dir: Option<PathBuf>,
}

/// Per-spawn config overrides for Codex: config.toml dotted keys to JSON values, sent as
/// `thread/start`'s (and `thread/resume`'s) `config`. Today: the repository's MCP servers
/// with the granted secrets they inherit by name ([`mcp::codex_config_for_spawn`]). Every
/// other server Codex would load from its own config is switched off by the adapter, and
/// the MCP Grants apply to these servers. Names only: secret values reach Codex through
/// its process environment ([`LaunchSpec::env`]), never through this config.
pub fn spawn_config(spec: &LaunchSpec) -> Map<String, Value> {
    mcp::codex_config_for_spawn(&spec.mcp_servers, &spec.env).into_iter().collect()
}

/// What the adapter is started with for `spec`.
pub fn launch_for(spec: &LaunchSpec) -> Launch {
    let rules = |list: &[String], allow: bool| -> Vec<McpRule> {
        list.iter()
            .filter_map(|rule| rule.strip_prefix("mcp:")?.split_once('/'))
            .map(|(server, tool)| McpRule { server: server.into(), tool: tool.into(), allow })
            .collect()
    };
    let mut mcp_rules = rules(&spec.allowed_tools, true);
    mcp_rules.extend(rules(&spec.disallowed_tools, false));
    Launch {
        cwd: spec.cwd.to_string_lossy().into_owned(),
        permission_mode: protocol_mode(spec.permission_mode, spec.sandboxed),
        model: spec.model.clone(),
        config: spawn_config(spec),
        mcp_servers: spec.mcp_servers.iter().map(|s| s.name.clone()).collect(),
        mcp_rules,
        list_only: spec.list_only,
    }
}

impl AgentLauncher for CodexLauncher {
    fn launch(&self, spec: LaunchSpec, sink: AgentSink) -> Result<Box<dyn AgentHandle>, String> {
        let adapter = Arc::new(Mutex::new(CodexAdapter::new(launch_for(&spec))));
        let mut command = Command::new(&self.codex);
        // Granted secrets in, every other known secret name out: Codex and the commands and
        // MCP servers it starts inherit this environment.
        spec.env.apply(&mut command);
        let mut child = command
            .arg("app-server")
            .current_dir(&spec.cwd)
            // Its own process group: after a crash, killing the recorded process takes the
            // commands and MCP servers Codex started with it.
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", self.codex.display()))?;

        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let pid = child.id();
        let child = Arc::new(Mutex::new(child));
        let record = |kind: &str| self.record_dir.as_ref().map(|dir| dir.join(format!("codex-{pid}.{kind}.jsonl")));
        let (record_input, record_writes) = (record("input"), record("writes"));

        // Writes go through a thread so neither the core nor the reader blocks on a full pipe.
        let (lines, inbox) = channel::<String>();
        thread::spawn(move || {
            for line in inbox {
                append(&record_writes, &line);
                if writeln!(stdin, "{line}").and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });

        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_reader = {
            let tail = tail.clone();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut tail = tail.lock().expect("stderr tail");
                    if tail.len() == STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            })
        };

        let (reader_adapter, reader_lines, waited) = (adapter.clone(), lines.clone(), child.clone());
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                append(&record_input, &line);
                // The adapter lock is released before the sink takes the core's.
                let (ingested, replies) = {
                    let mut adapter = reader_adapter.lock().expect("adapter");
                    let ingested = adapter.ingest(&line);
                    (ingested, adapter.replies())
                };
                for reply in replies {
                    let _ = reader_lines.send(reply);
                }
                match ingested {
                    Ok(events) => events.into_iter().for_each(|event| sink(AgentOutput::Event(event))),
                    Err(_) => sink(AgentOutput::Unreadable { line }),
                }
            }
            let _ = stderr_reader.join();
            let code = waited.lock().expect("child").wait().ok().and_then(|s| s.code());
            let stderr = Vec::from(tail.lock().expect("stderr tail").clone()).join("\n");
            sink(AgentOutput::Exited { code, stderr });
        });

        Ok(Box::new(CodexHandle { adapter, lines, child, pid }))
    }
}

fn append(file: &Option<PathBuf>, line: &str) {
    if let Some(path) = file {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

struct CodexHandle {
    adapter: Arc<Mutex<CodexAdapter>>,
    lines: Sender<String>,
    child: Arc<Mutex<Child>>,
    pid: u32,
}

impl AgentHandle for CodexHandle {
    fn send(&mut self, command: adapter_protocol::Command) -> Result<(), String> {
        let writes = self.adapter.lock().expect("adapter").command(&command).map_err(|e| e.0)?;
        for line in writes {
            self.lines.send(line).map_err(|_| "the agent process has exited".to_string())?;
        }
        Ok(())
    }

    fn pid(&self) -> Option<u32> {
        Some(self.pid)
    }
}

impl Drop for CodexHandle {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.kill();
        }
    }
}
