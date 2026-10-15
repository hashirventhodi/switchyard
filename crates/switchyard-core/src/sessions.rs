//! Sessions: one agent on one worktree. Owns the session state machine (the only
//! transitions the core makes are listed on [`transition`]) and each session's transcript.
//! Sessions live in memory for now; persistence and recovery come later.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use adapter_protocol::{Command as AgentCommand, Event as AgentEvent};
use serde_json::Value;
use switchyard_schema::{
    AgentKind, Entry, Event, PermissionMode, Session, SessionFacts, SessionStatus, ToolState,
};

use crate::agents::{AgentHandle, AgentOutput, AgentSink, LaunchSpec};
use crate::Inner;

#[derive(Default)]
pub(crate) struct Sessions {
    list: Vec<SessionState>,
    next_id: u64,
}

struct SessionState {
    session: Session,
    facts: SessionFacts,
    transcript: Vec<Entry>,
    /// Transcript index of each tool call, by call id.
    tool_calls: HashMap<String, usize>,
    agent: Box<dyn AgentHandle>,
}

/// Why the status changes. Each variant is one row of the spec's state machine.
enum Cause {
    /// A permission request, a question or a finished turn.
    NeedsUser,
    /// The agent declared its work complete.
    AgentDone,
    /// The process exited or the CLI reported a fatal error.
    AgentFailed,
    /// The user pressed Stop / Esc.
    UserStop,
    /// The user sent a prompt (or answered a request).
    UserMessage,
}

/// The state machine. Returns the new status, or `None` when `cause` does not move a
/// session in `from`:
///
/// ```text
/// running  -> waiting   (permission request | question | turn complete)
/// running  -> failed    (process exit | fatal error)
/// running  -> done      (agent declares complete)
/// running  -> idle      (user Stop / Esc)
/// waiting  -> running   (request answered | user message)
/// waiting  -> idle      (user Stop)
/// waiting  -> done      (agent declares complete after its turn ended)
/// waiting  -> failed    (process exit | fatal error)
/// failed   -> running   (user message)
/// done     -> running   (user message)
/// idle     -> running   (Resume; not yet built)
/// ```
fn transition(from: SessionStatus, cause: &Cause) -> Option<SessionStatus> {
    use SessionStatus::*;
    match (from, cause) {
        (Running, Cause::NeedsUser) => Some(Waiting),
        (Running | Waiting, Cause::AgentDone) => Some(Done),
        (Running | Waiting, Cause::AgentFailed) => Some(Failed),
        (Running | Waiting, Cause::UserStop) => Some(Idle),
        (Waiting | Failed | Done, Cause::UserMessage) => Some(Running),
        _ => None,
    }
}

pub(crate) fn start(
    inner: &mut Inner,
    core: &Arc<Mutex<Inner>>,
    worktree_path: &str,
    agent: AgentKind,
    permission_mode: PermissionMode,
    prompt: &str,
) {
    const COMMAND: &str = "startSession";
    if prompt.trim().is_empty() {
        return inner.refuse(COMMAND, "Write a prompt for the agent first".into());
    }
    let Some((repo, worktree)) = inner.find_worktree(worktree_path) else {
        return inner.refuse(COMMAND, format!("{worktree_path} is not a worktree of an added repository"));
    };
    let name = worktree.branch.clone().unwrap_or_else(|| worktree_path.to_string());
    if worktree.primary && permission_mode != PermissionMode::Explore {
        return inner.refuse(
            COMMAND,
            format!("{name} is the primary worktree: only Explore sessions may run there"),
        );
    }
    // Exactly one session per worktree, of any kind (ADR-0005).
    if let Some(occupant) = inner.sessions.list.iter().find(|s| s.session.worktree_path == worktree_path) {
        let label = status_label(occupant.session.status);
        let reason = match occupant.session.status {
            SessionStatus::Running | SessionStatus::Waiting => {
                format!("{name} already has a {label} session; stop it or pick another worktree")
            }
            _ => format!("{name} already has a {label} session; replacing it is not available yet"),
        };
        return inner.refuse(COMMAND, reason);
    }
    let id = format!("session-{}", inner.sessions.next_id + 1);
    let spec = LaunchSpec { agent, cwd: worktree.path.clone(), permission_mode };
    let handle = match inner.runtime.agents.launch(spec, sink(Arc::downgrade(core), id.clone())) {
        Ok(handle) => handle,
        Err(reason) => return inner.refuse(COMMAND, format!("Could not start {}: {reason}", agent_name(agent))),
    };
    inner.sessions.next_id += 1;
    let session = Session {
        id: id.clone(),
        repository_id: repo.id,
        worktree_path: worktree_path.into(),
        branch: worktree.branch,
        agent,
        permission_mode,
        status: SessionStatus::Running,
        started_at_ms: (inner.runtime.now_ms)(),
    };
    inner.emit(Event::SessionStarted { session: session.clone() });
    inner.sessions.list.push(SessionState {
        session,
        facts: SessionFacts::default(),
        transcript: Vec::new(),
        tool_calls: HashMap::new(),
        agent: handle,
    });
    let state = inner.sessions.list.last_mut().expect("just pushed");
    let entry = append(state, Entry::User { text: prompt.into() });
    inner.emit(entry);
    send_to_agent(inner, &id, AgentCommand::SendText { text: prompt.into() });
}

pub(crate) fn stop(inner: &mut Inner, id: &str) {
    const COMMAND: &str = "stopSession";
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else {
        return inner.refuse(COMMAND, format!("There is no session {id}"));
    };
    let Some(status) = change_status(state, &Cause::UserStop) else {
        return inner.refuse(COMMAND, "The session is not running".into());
    };
    let entry = append(state, Entry::System { text: "Stopped by you.".into() });
    inner.emit(status);
    inner.emit(entry);
    // Interrupting ends the CLI's turn; it never touches the worktree's files.
    send_to_agent(inner, id, AgentCommand::Interrupt);
}

pub(crate) fn send_prompt(inner: &mut Inner, id: &str, text: &str) {
    const COMMAND: &str = "sendPrompt";
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else {
        return inner.refuse(COMMAND, format!("There is no session {id}"));
    };
    let Some(status) = change_status(state, &Cause::UserMessage) else {
        let reason = match state.session.status {
            SessionStatus::Running => "The agent is still working; wait for it or stop it first",
            _ => "The session is paused; resume it first",
        };
        return inner.refuse(COMMAND, reason.into());
    };
    let entry = append(state, Entry::User { text: text.into() });
    inner.emit(status);
    inner.emit(entry);
    send_to_agent(inner, id, AgentCommand::SendText { text: text.into() });
}

/// Emit every session, its facts and its transcript (on attach).
pub(crate) fn replay(inner: &Inner) {
    for state in &inner.sessions.list {
        let id = &state.session.id;
        inner.emit(Event::SessionStarted { session: state.session.clone() });
        inner.emit(Event::SessionFactsChanged { session_id: id.clone(), facts: state.facts.clone() });
        for (index, entry) in state.transcript.iter().enumerate() {
            inner.emit(Event::TranscriptEntry { session_id: id.clone(), index: index as u32, entry: entry.clone() });
        }
    }
}

fn sink(core: Weak<Mutex<Inner>>, session_id: String) -> AgentSink {
    Arc::new(move |output| {
        if let Some(core) = core.upgrade() {
            let mut inner = core.lock().expect("core lock");
            on_agent(&mut inner, &session_id, output);
        }
    })
}

fn on_agent(inner: &mut Inner, id: &str, output: AgentOutput) {
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else { return };
    let mut out = Vec::new();
    let mut cause = None;
    match output {
        AgentOutput::Event(event) => match event {
            AgentEvent::SessionInit { cli_permission_mode, cli, model, .. } => {
                state.facts.model = model;
                state.facts.cli_version = Some(cli.version);
                state.facts.cli_permission_mode = Some(cli_permission_mode);
                out.push(facts(state));
            }
            AgentEvent::AgentText { text } => out.push(append(state, Entry::Agent { text })),
            AgentEvent::ToolCallStarted { call_id, tool, input } => {
                let summary = summarise(&tool, &input);
                state.tool_calls.insert(call_id.clone(), state.transcript.len());
                out.push(append(state, Entry::Tool { call_id, tool, summary, state: ToolState::Running, output: None }));
            }
            AgentEvent::ToolCallFinished { call_id, ok, output } => {
                if let Some(&index) = state.tool_calls.get(&call_id) {
                    if let Entry::Tool { state: tool_state, output: tool_output, .. } = &mut state.transcript[index] {
                        *tool_state = if ok { ToolState::Ok } else { ToolState::Failed };
                        *tool_output = output;
                    }
                    out.push(Event::TranscriptEntry {
                        session_id: id.into(),
                        index: index as u32,
                        entry: state.transcript[index].clone(),
                    });
                }
            }
            AgentEvent::PermissionRequest { .. } => cause = Some(Cause::NeedsUser),
            AgentEvent::TurnComplete => {
                state.facts.turns += 1;
                out.push(facts(state));
                cause = Some(Cause::NeedsUser);
            }
            AgentEvent::Done { .. } => cause = Some(Cause::AgentDone),
            AgentEvent::Error { message, fatal } => {
                out.push(append(state, Entry::System { text: format!("Agent error: {message}") }));
                if fatal {
                    cause = Some(Cause::AgentFailed);
                }
            }
            AgentEvent::TokenUsage { cost_usd, .. } => {
                if cost_usd.is_some() {
                    state.facts.cost_usd = cost_usd;
                    out.push(facts(state));
                }
            }
            AgentEvent::ClassifierDenial { tool, detail, .. } => {
                out.push(append(state, Entry::System { text: format!("Denied by the classifier: {tool} {detail}") }))
            }
        },
        AgentOutput::Exited { code, stderr } => {
            let code = code.map_or("a signal".to_string(), |c| format!("code {c}"));
            let mut text = format!("The agent process exited ({code}).");
            if !stderr.trim().is_empty() {
                text.push('\n');
                text.push_str(stderr.trim());
            }
            out.push(append(state, Entry::System { text }));
            cause = Some(Cause::AgentFailed);
        }
    }
    if let Some(event) = cause.and_then(|c| change_status(state, &c)) {
        out.push(event);
    }
    for event in out {
        inner.emit(event);
    }
}

fn change_status(state: &mut SessionState, cause: &Cause) -> Option<Event> {
    let next = transition(state.session.status, cause)?;
    state.session.status = next;
    Some(Event::SessionStatusChanged { session_id: state.session.id.clone(), status: next })
}

fn send_to_agent(inner: &mut Inner, id: &str, command: AgentCommand) {
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else { return };
    if let Err(reason) = state.agent.send(command) {
        let entry = append(state, Entry::System { text: format!("Could not reach the agent: {reason}") });
        inner.emit(entry);
    }
}

fn append(state: &mut SessionState, entry: Entry) -> Event {
    let index = state.transcript.len() as u32;
    state.transcript.push(entry.clone());
    Event::TranscriptEntry { session_id: state.session.id.clone(), index, entry }
}

fn facts(state: &SessionState) -> Event {
    Event::SessionFactsChanged { session_id: state.session.id.clone(), facts: state.facts.clone() }
}

/// The label every surface shows for a status.
fn status_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "Running",
        SessionStatus::Waiting => "Needs you",
        SessionStatus::Idle => "Paused",
        SessionStatus::Done => "Finished",
        SessionStatus::Failed => "Error",
    }
}

fn agent_name(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::ClaudeCode => "Claude Code",
    }
}

/// One line describing a tool call: the command, the path, the pattern...
fn summarise(tool: &str, input: &Value) -> String {
    let field = |name: &str| input.get(name).and_then(Value::as_str).map(str::to_owned);
    let summary = match tool {
        "Bash" => field("command"),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => field("file_path").or_else(|| field("notebook_path")),
        "Grep" | "Glob" => field("pattern").map(|p| match field("path") {
            Some(path) => format!("{p} in {path}"),
            None => p,
        }),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        "Task" | "Agent" => field("description"),
        _ => None,
    };
    let mut summary = summary.unwrap_or_else(|| input.to_string());
    if let Some(first_line) = summary.lines().next() {
        if first_line.len() < summary.len() {
            summary = format!("{first_line} …");
        }
    }
    if summary.chars().count() > 200 {
        summary = summary.chars().take(200).collect::<String>() + "…";
    }
    summary
}
