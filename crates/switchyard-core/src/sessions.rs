//! Sessions: one agent on one worktree. Owns the session state machine (the only
//! transitions the core makes are listed on [`transition`]) and each session's transcript.
//! Every session is written through to the store ([`flush`], after each command and each
//! agent output); on open the core reloads them and recovers from a crash (`recovery`).
//! Replaced sessions are archived (`archive`).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};

use adapter_protocol::{Command as AgentCommand, Event as AgentEvent};
use serde_json::Value;
use switchyard_schema::{
    AgentKind, ArchivedSession, Entry, Event, PermissionMode, Session, SessionFacts, SessionStatus, Settings, ToolState,
    Verdict,
};

use crate::spend::{PriceTable, Usage};
use crate::agents::{AgentHandle, AgentOutput, AgentSink, LaunchSpec};
use crate::compat::{self, FailureClass};
use crate::processes::{self, Recorded};
use crate::session_store::{self, Row, CHUNK};
use crate::policy;
use crate::Inner;

mod archive;
mod recovery;
mod requests;
pub(crate) use archive::{archive_occupant, export, read_archived, replace, update_settings};
pub(crate) use recovery::load;
pub(crate) use requests::answer as answer_request;

/// What the agent is told when the user presses Resume.
const CONTINUE: &str = "Continue where you left off.";

#[derive(Default)]
pub(crate) struct Sessions {
    list: Vec<SessionState>,
    next_id: u64,
    /// Archived sessions (without their transcripts, which stay in the store), oldest first.
    archived: Vec<ArchivedSession>,
    settings: Settings,
}

struct SessionState {
    session: Session,
    facts: SessionFacts,
    transcript: Vec<Entry>,
    /// Transcript index of each tool call, by call id.
    tool_calls: HashMap<String, usize>,
    /// Every protocol event the agent sent, as JSON, for the Agent log's debug view.
    raw_events: Vec<String>,
    /// Transcript index of each Permission request, by request id.
    requests: HashMap<String, usize>,
    /// Every token_usage report, oldest first. Each holds running totals.
    usage: Vec<Usage>,
    /// The running agent; `None` once its process is gone (exited, or a previous run of
    /// the app started it). Talking to the agent again relaunches it by `external_id`.
    agent: Option<Box<dyn AgentHandle>>,
    /// The CLI's own session id (from session_init), for resume-by-id.
    external_id: Option<String>,
    /// The agent process, recorded so a relaunch after a crash can kill it.
    process: Option<Recorded>,
    saved: Saved,
}

/// The stored row's changing parts: status, facts, external id, process, entry count.
type RowFacts = (SessionStatus, SessionFacts, Option<String>, Option<Recorded>, usize);

/// What of a session is already in the store.
#[derive(Default)]
struct Saved {
    /// The row as last written; `None` before the first write.
    row: Option<RowFacts>,
    /// Transcript entries written (entries replaced since are in `chunks`).
    entries: usize,
    /// Chunks holding an entry replaced in place since the last write.
    chunks: BTreeSet<usize>,
    usage: usize,
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
    /// The user sent a prompt.
    UserMessage,
    /// The user answered the last pending Permission request.
    RequestAnswered,
    /// The user pressed Resume.
    Resume,
    /// The app was quit or crashed while the agent worked; on relaunch it is Paused.
    Recovered,
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
/// done     -> running   (user message | Resume)
/// idle     -> running   (Resume)
/// running  -> idle      (crash recovery: the app was gone)
/// waiting  -> idle      (crash recovery)
/// ```
fn transition(from: SessionStatus, cause: &Cause) -> Option<SessionStatus> {
    use SessionStatus::*;
    match (from, cause) {
        (Running, Cause::NeedsUser) => Some(Waiting),
        (Running | Waiting, Cause::AgentDone) => Some(Done),
        (Running | Waiting, Cause::AgentFailed) => Some(Failed),
        (Running | Waiting, Cause::UserStop) => Some(Idle),
        (Waiting | Failed | Done, Cause::UserMessage) => Some(Running),
        (Waiting, Cause::RequestAnswered) => Some(Running),
        (Idle | Done, Cause::Resume) => Some(Running),
        (Running | Waiting, Cause::Recovered) => Some(Idle),
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
    begin(inner, core, "startSession", worktree_path, agent, permission_mode, prompt, None);
}

/// Start a session on a worktree; when `replacing` names its (Paused, Finished or Error)
/// occupant, that one is archived once the new agent has started.
#[allow(clippy::too_many_arguments)]
fn begin(
    inner: &mut Inner,
    core: &Arc<Mutex<Inner>>,
    command: &str,
    worktree_path: &str,
    agent: AgentKind,
    permission_mode: PermissionMode,
    prompt: &str,
    replacing: Option<&str>,
) {
    let refuse = |inner: &Inner, reason: String| inner.refuse(command, reason);
    if prompt.trim().is_empty() {
        return refuse(inner, "Write a prompt for the agent first".into());
    }
    let Some((repo, worktree)) = inner.find_worktree(worktree_path) else {
        return refuse(inner, format!("{worktree_path} is not a worktree of an added repository"));
    };
    let name = worktree.branch.clone().unwrap_or_else(|| worktree_path.to_string());
    if worktree.primary && permission_mode != PermissionMode::Explore {
        return refuse(inner, format!("{name} is the primary worktree: only Explore sessions may run there"));
    }
    // Exactly one session per worktree, of any kind (ADR-0005).
    let occupant = inner
        .sessions
        .list
        .iter()
        .find(|s| s.session.worktree_path == worktree_path && Some(s.session.id.as_str()) != replacing);
    if let Some(occupant) = occupant {
        let label = status_label(occupant.session.status);
        let reason = match occupant.session.status {
            SessionStatus::Running | SessionStatus::Waiting => {
                format!("{name} already has a {label} session; stop it or pick another worktree")
            }
            _ => format!("{name} already has a {label} session; stop and replace it, or pick another worktree"),
        };
        return refuse(inner, reason);
    }
    if let Err(reason) = policy::admit_mode(&repo, permission_mode) {
        return refuse(inner, reason);
    }
    // Before the first session, not mid-session: an unsupported CLI never starts (#13).
    if let Err(reason) = compat::admit(inner, agent) {
        return refuse(inner, reason);
    }
    let id = format!("session-{}", inner.sessions.next_id + 1);
    // Grants reach the CLI as flags; the CLI's own settings files are never written.
    let (allowed_tools, disallowed_tools) = policy::cli_rules(agent, &policy::resolve(inner, &repo));
    let spec = LaunchSpec { agent, cwd: worktree.path.clone(), permission_mode, allowed_tools, disallowed_tools };
    let handle = match inner.runtime.agents.launch(spec, sink(Arc::downgrade(core), id.clone())) {
        Ok(handle) => handle,
        Err(reason) => {
            compat::record(inner, agent, FailureClass::SpawnFailure, None, &reason);
            return refuse(inner, format!("Could not start {}: {reason}", agent_name(agent)));
        }
    };
    let process = processes::record(&*inner.runtime.processes, handle.pid());
    if let Some(old) = replacing {
        archive::archive(inner, old);
    }
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
        raw_events: Vec::new(),
        requests: HashMap::new(),
        usage: Vec::new(),
        agent: Some(handle),
        external_id: None,
        process,
        saved: Saved::default(),
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
    let cancelled = requests::cancel_all(state);
    inner.emit(status);
    inner.emit(entry);
    for event in cancelled {
        inner.emit(event);
    }
    // Interrupting ends the CLI's turn; it never touches the worktree's files.
    send_to_agent(inner, id, AgentCommand::Interrupt);
}

pub(crate) fn send_prompt(inner: &mut Inner, core: &Arc<Mutex<Inner>>, id: &str, text: &str) {
    const COMMAND: &str = "sendPrompt";
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else {
        return inner.refuse(COMMAND, format!("There is no session {id}"));
    };
    // While the agent asks something, what the user types is the answer to its question.
    if let Some(question) = requests::pending_question(state) {
        return requests::resolve(inner, id, &question, Verdict::Answer { text: text.into() });
    }
    if requests::has_pending(state) {
        return inner.refuse(COMMAND, "Answer the agent's request first: Run once or Skip".into());
    }
    if transition(state.session.status, &Cause::UserMessage).is_none() {
        let reason = match state.session.status {
            SessionStatus::Running => "The agent is still working; wait for it or stop it first",
            _ => "The session is paused; resume it first",
        };
        return inner.refuse(COMMAND, reason.into());
    }
    if let Err(reason) = ensure_agent(inner, core, id) {
        return inner.refuse(COMMAND, reason);
    }
    let Some(state) = find(inner, id) else { return };
    let Some(status) = change_status(state, &Cause::UserMessage) else { return };
    let entry = append(state, Entry::User { text: text.into() });
    inner.emit(status);
    inner.emit(entry);
    send_to_agent(inner, id, AgentCommand::SendText { text: text.into() });
}

/// The `resumeSession` command: a Paused or Finished session continues where it left off.
/// A live agent is simply told to continue; one whose process is gone is relaunched first,
/// through its CLI's resume-by-id, so it keeps its context.
pub(crate) fn resume(inner: &mut Inner, core: &Arc<Mutex<Inner>>, id: &str) {
    const COMMAND: &str = "resumeSession";
    let Some(state) = find(inner, id) else {
        return inner.refuse(COMMAND, format!("There is no session {id}"));
    };
    if transition(state.session.status, &Cause::Resume).is_none() {
        let label = status_label(state.session.status);
        return inner.refuse(COMMAND, format!("A {label} session cannot be resumed"));
    }
    if let Err(reason) = ensure_agent(inner, core, id) {
        return inner.refuse(COMMAND, reason);
    }
    let Some(state) = find(inner, id) else { return };
    let Some(status) = change_status(state, &Cause::Resume) else { return };
    let entry = append(state, Entry::System { text: "Resumed by you.".into() });
    inner.emit(status);
    inner.emit(entry);
    send_to_agent(inner, id, AgentCommand::SendText { text: CONTINUE.into() });
}

/// Make sure the session has a running agent: relaunch a gone one by its CLI session id.
fn ensure_agent(inner: &mut Inner, core: &Arc<Mutex<Inner>>, id: &str) -> Result<(), String> {
    let Some(state) = find(inner, id) else { return Err(format!("There is no session {id}")) };
    if state.agent.is_some() {
        return Ok(());
    }
    let agent = state.session.agent;
    let Some(external_id) = state.external_id.clone() else {
        return Err(format!(
            "There is nothing to resume: {} never started for this session; start a new one",
            agent_name(agent)
        ));
    };
    let (cwd, permission_mode) = (state.session.worktree_path.clone(), state.session.permission_mode);
    let repository_id = state.session.repository_id.clone();
    // Grants as they are now: some may have been made or revoked since the session began.
    let repository = policy::repository(inner, &repository_id);
    let (allowed_tools, disallowed_tools) = match repository {
        Some(repo) => policy::cli_rules(agent, &policy::resolve(inner, &repo)),
        None => Default::default(),
    };
    let spec = LaunchSpec { agent, cwd: cwd.into(), permission_mode, allowed_tools, disallowed_tools };
    let mut handle = match inner.runtime.agents.launch(spec, sink(Arc::downgrade(core), id.to_string())) {
        Ok(handle) => handle,
        Err(reason) => {
            compat::record(inner, agent, FailureClass::SpawnFailure, None, &reason);
            return Err(format!("Could not resume {}: {reason}", agent_name(agent)));
        }
    };
    let process = processes::record(&*inner.runtime.processes, handle.pid());
    let sent = handle.send(AgentCommand::Resume { external_session_id: external_id });
    let state = find(inner, id).expect("the session is still there");
    state.agent = Some(handle);
    state.process = process;
    sent.map_err(|reason| format!("Could not resume {}: {reason}", agent_name(agent)))
}

/// Write every session's changes to the store. Called after each command and each agent
/// output, so a crash loses at most the event in flight.
pub(crate) fn flush(inner: &mut Inner) {
    let Inner { store, sessions, .. } = inner;
    for state in &mut sessions.list {
        let _ = persist(store.db(), state, None);
    }
}

/// Write what changed in `state` (with `archived_at_ms` when it is being archived).
fn persist(db: &rusqlite::Connection, state: &mut SessionState, archived_at_ms: Option<u64>) -> rusqlite::Result<()> {
    let row = (
        state.session.status,
        state.facts.clone(),
        state.external_id.clone(),
        state.process.clone(),
        state.transcript.len(),
    );
    let mut chunks = std::mem::take(&mut state.saved.chunks);
    if state.saved.entries < state.transcript.len() {
        chunks.extend(state.saved.entries / CHUNK..=(state.transcript.len() - 1) / CHUNK);
    }
    let row_changed = state.saved.row.as_ref() != Some(&row) || archived_at_ms.is_some();
    if !row_changed && chunks.is_empty() && state.saved.usage == state.usage.len() {
        return Ok(());
    }
    let id = state.session.id.clone();
    let result = session_store::batch(db, |db| {
        if row_changed {
            session_store::save_row(
                db,
                &Row {
                    session: state.session.clone(),
                    facts: state.facts.clone(),
                    external_id: state.external_id.clone(),
                    process: state.process.clone(),
                    entries: state.transcript.len(),
                    archived_at_ms,
                },
            )?;
        }
        for &chunk in &chunks {
            let start = chunk * CHUNK;
            let end = (start + CHUNK).min(state.transcript.len());
            session_store::save_chunk(db, &id, chunk, &state.transcript[start..end])?;
        }
        for (seq, usage) in state.usage.iter().enumerate().skip(state.saved.usage) {
            session_store::save_usage(db, &id, seq, usage)?;
        }
        Ok(())
    });
    match result {
        Ok(()) => {
            state.saved.row = Some(row);
            state.saved.entries = state.transcript.len();
            state.saved.usage = state.usage.len();
        }
        Err(_) => state.saved.chunks = chunks, // written again on the next flush
    }
    result
}

/// The worktree path and status of session `id`, if it exists.
pub(crate) fn worktree_and_status(inner: &Inner, id: &str) -> Option<(String, SessionStatus)> {
    let state = inner.sessions.list.iter().find(|s| s.session.id == id)?;
    Some((state.session.worktree_path.clone(), state.session.status))
}

/// True when the session on the worktree at `path` is waiting on a Permission request or
/// question: mid-turn, so its tree must not move under it.
pub(crate) fn awaits_request(inner: &Inner, path: &str) -> bool {
    inner.sessions.list.iter().any(|s| s.session.worktree_path == path && requests::has_pending(s))
}

/// The status of the session occupying the worktree at `path`, if any.
pub(crate) fn occupant(inner: &Inner, path: &str) -> Option<SessionStatus> {
    inner.sessions.list.iter().find(|s| s.session.worktree_path == path).map(|s| s.session.status)
}

/// Emit every session, its facts, its spend (priced now) and its transcript (on attach).
pub(crate) fn replay(inner: &Inner) {
    let table = PriceTable::read(&inner.data_dir);
    for state in &inner.sessions.list {
        let id = &state.session.id;
        inner.emit(Event::SessionStarted { session: state.session.clone() });
        inner.emit(Event::SessionFactsChanged { session_id: id.clone(), facts: state.facts.clone() });
        for usage in &state.usage {
            inner.emit(spend_event(state, usage, &table));
        }
        for (index, entry) in state.transcript.iter().enumerate() {
            inner.emit(Event::TranscriptEntry { session_id: id.clone(), index: index as u32, entry: entry.clone() });
        }
        for (index, json) in state.raw_events.iter().enumerate() {
            inner.emit(Event::AgentEventReceived { session_id: id.clone(), index: index as u32, json: json.clone() });
        }
    }
    inner.emit(Event::SettingsChanged { settings: inner.sessions.settings.clone() });
    for archived in &inner.sessions.archived {
        inner.emit(Event::SessionArchived { archived: archived.clone() });
    }
}

fn sink(core: Weak<Mutex<Inner>>, session_id: String) -> AgentSink {
    Arc::new(move |output| {
        if let Some(core) = core.upgrade() {
            let mut inner = core.lock().expect("core lock");
            on_agent(&mut inner, &session_id, output);
            flush(&mut inner);
        }
    })
}

fn find<'a>(inner: &'a mut Inner, id: &str) -> Option<&'a mut SessionState> {
    inner.sessions.list.iter_mut().find(|s| s.session.id == id)
}

fn on_agent(inner: &mut Inner, id: &str, output: AgentOutput) {
    if let AgentOutput::Event(AgentEvent::PermissionRequest { request_id, request, reason }) = output {
        return requests::arrived(inner, id, request_id, request, reason);
    }
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else { return };
    let mut out = Vec::new();
    let mut cause = None;
    if let AgentOutput::Event(event) = &output {
        out.push(record_raw(state, event));
    }
    let mut failure = None;
    let turn_ended = matches!(output, AgentOutput::Event(AgentEvent::TurnComplete | AgentEvent::Done { .. }));
    match output {
        AgentOutput::Event(event) => match event {
            AgentEvent::SessionInit { external_session_id, cli_permission_mode, cli, model, .. } => {
                state.external_id = Some(external_session_id);
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
                    replaced(state, index);
                    out.push(Event::TranscriptEntry {
                        session_id: id.into(),
                        index: index as u32,
                        entry: state.transcript[index].clone(),
                    });
                }
            }
            AgentEvent::PermissionRequest { .. } => {} // handled above, by requests::arrived
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
            AgentEvent::TokenUsage { input_tokens, output_tokens, cache_read_tokens, cost_usd } => {
                let usage = Usage {
                    at_ms: (inner.runtime.now_ms)(),
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    reported_usd: cost_usd,
                };
                state.usage.push(usage);
                let table = PriceTable::read(&inner.data_dir);
                out.push(spend_event(state, &usage, &table));
            }
            AgentEvent::ClassifierDenial { tool, detail, .. } => {
                out.push(append(state, Entry::System { text: format!("Denied by the classifier: {tool} {detail}") }))
            }
        },
        AgentOutput::Unreadable { line } => {
            failure = Some((compat::classify_unreadable(&line), format!("unreadable adapter output: {line}")));
            out.push(append(state, Entry::System { text: format!("Agent error: the adapter wrote an unreadable line: {line}") }));
        }
        AgentOutput::Exited { code, stderr } => {
            let code = code.map_or("a signal".to_string(), |c| format!("code {c}"));
            let mut text = format!("The agent process exited ({code}).");
            if !stderr.trim().is_empty() {
                text.push('\n');
                text.push_str(stderr.trim());
            }
            // Ending before the CLI even initialised is the adapter failing to start.
            if state.facts.cli_version.is_none() {
                failure = Some((FailureClass::SpawnFailure, format!("exited before session_init: {text}")));
            }
            out.push(append(state, Entry::System { text }));
            cause = Some(Cause::AgentFailed);
            // Gone: the next prompt or Resume relaunches it by its CLI session id.
            state.agent = None;
            state.process = None;
        }
    }
    if let Some(event) = cause.and_then(|c| change_status(state, &c)) {
        out.push(event);
    }
    let (agent, cli_version) = (state.session.agent, state.facts.cli_version.clone());
    let worktree_path = state.session.worktree_path.clone();
    for event in out {
        inner.emit(event);
    }
    // A turn is when the agent commits and edits: re-read where its branch stands.
    if turn_ended {
        crate::worktrees::refresh_one(inner, &worktree_path);
    }
    if let Some((class, detail)) = failure {
        compat::record(inner, agent, class, cli_version.as_deref(), &detail);
    }
}

fn record_raw(state: &mut SessionState, event: &AgentEvent) -> Event {
    let json = serde_json::to_string(event).unwrap_or_else(|e| format!("{{\"unserialisable\":\"{e}\"}}"));
    let index = state.raw_events.len() as u32;
    state.raw_events.push(json.clone());
    Event::AgentEventReceived { session_id: state.session.id.clone(), index, json }
}

fn change_status(state: &mut SessionState, cause: &Cause) -> Option<Event> {
    let next = transition(state.session.status, cause)?;
    state.session.status = next;
    Some(Event::SessionStatusChanged { session_id: state.session.id.clone(), status: next })
}

fn send_to_agent(inner: &mut Inner, id: &str, command: AgentCommand) {
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else { return };
    let Some(agent) = state.agent.as_mut() else { return };
    if let Err(reason) = agent.send(command) {
        let entry = append(state, Entry::System { text: format!("Could not reach the agent: {reason}") });
        inner.emit(entry);
    }
}

fn append(state: &mut SessionState, entry: Entry) -> Event {
    let index = state.transcript.len() as u32;
    state.transcript.push(entry.clone());
    Event::TranscriptEntry { session_id: state.session.id.clone(), index, entry }
}

/// Entry `index` was changed in place: its chunk must be written again.
fn replaced(state: &mut SessionState, index: usize) {
    if index < state.saved.entries {
        state.saved.chunks.insert(index / CHUNK);
    }
}

fn spend_event(state: &SessionState, usage: &Usage, table: &PriceTable) -> Event {
    let spend = usage.spend(state.facts.model.as_deref(), table);
    Event::SpendRecorded { session_id: state.session.id.clone(), at_ms: usage.at_ms, spend }
}

fn facts(state: &SessionState) -> Event {
    Event::SessionFactsChanged { session_id: state.session.id.clone(), facts: state.facts.clone() }
}

/// The label every surface shows for a status.
pub(crate) fn status_label(status: SessionStatus) -> &'static str {
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
