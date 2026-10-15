//! Runs an agent as a sidecar process that speaks the adapter protocol on stdio: one JSON
//! protocol command per line on its stdin, one JSON protocol event per line on its stdout,
//! diagnostics on stderr. The Claude Code sidecar (`sidecar/`, a bundled Node script around
//! the Agent SDK) is the first; see `sidecar/README.md`.
//!
//! The sidecar is started as `<node> <script> --config <json>` in the worktree, where the
//! config is `{"cwd", "permission_mode" (protocol vocabulary), "claude_path" (or null),
//! "allowed_tools", "disallowed_tools" (Claude Code permission rules from the resolved
//! Policy), "allow_bypass" (the repository's policy declares a sandbox), "mcp_servers" (the
//! repository's MCP servers as the SDK's `mcpServers` map; the sidecar always runs the CLI
//! with `--strict-mcp-config`, so these are the only servers it loads)}`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;

use crate::agents::{AgentHandle, AgentLauncher, AgentOutput, AgentSink, LaunchSpec};

/// Lines of stderr kept for the transcript when the process exits.
const STDERR_TAIL: usize = 20;

pub struct SidecarLauncher {
    /// The Node binary (pinned in release builds, `node` on PATH in development).
    pub node: PathBuf,
    /// The bundled sidecar script.
    pub script: PathBuf,
    /// The `claude` executable the SDK should run; `None` lets the sidecar find it.
    pub claude: Option<PathBuf>,
}

impl AgentLauncher for SidecarLauncher {
    fn launch(&self, spec: LaunchSpec, sink: AgentSink) -> Result<Box<dyn AgentHandle>, String> {
        let config = json!({
            "cwd": spec.cwd,
            "permission_mode": crate::agents::protocol_mode(spec.permission_mode, spec.sandboxed),
            "claude_path": self.claude,
            "allowed_tools": spec.allowed_tools,
            "disallowed_tools": spec.disallowed_tools,
            "allow_bypass": spec.sandboxed,
            "mcp_servers": crate::mcp::claude_servers(&spec.mcp_servers),
        });
        let mut child = Command::new(&self.node)
            .arg(&self.script)
            .arg("--config")
            .arg(config.to_string())
            .current_dir(&spec.cwd)
            // Its own process group: after a crash, killing the recorded sidecar takes the CLI
            // it started with it.
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", self.node.display()))?;

        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let pid = child.id();
        let child = Arc::new(Mutex::new(child));

        // Writes go through a thread so the core never blocks on a full pipe.
        let (commands, inbox) = channel::<adapter_protocol::Command>();
        thread::spawn(move || {
            for command in inbox {
                let line = serde_json::to_string(&command).expect("protocol commands serialize");
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

        let waited = child.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str(&line) {
                    Ok(event) => sink(AgentOutput::Event(event)),
                    Err(_) => sink(AgentOutput::Unreadable { line }),
                }
            }
            let _ = stderr_reader.join();
            let code = waited.lock().expect("child").wait().ok().and_then(|s| s.code());
            let stderr = Vec::from(tail.lock().expect("stderr tail").clone()).join("\n");
            sink(AgentOutput::Exited { code, stderr });
        });

        Ok(Box::new(SidecarHandle { commands, child, pid }))
    }
}

struct SidecarHandle {
    commands: Sender<adapter_protocol::Command>,
    child: Arc<Mutex<Child>>,
    pid: u32,
}

impl AgentHandle for SidecarHandle {
    fn send(&mut self, command: adapter_protocol::Command) -> Result<(), String> {
        self.commands.send(command).map_err(|_| "the agent process has exited".to_string())
    }

    fn pid(&self) -> Option<u32> {
        Some(self.pid)
    }
}

impl Drop for SidecarHandle {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.kill();
        }
    }
}

