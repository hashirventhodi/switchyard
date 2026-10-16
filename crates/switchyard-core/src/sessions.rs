//! Sessions: one agent on one worktree. Owns the session state machine (the only
//! transitions the core makes are listed on [`transition`]) and each session's transcript.
//! Every session is written through to the store ([`flush`], after each command and each
//! agent output); on open the core reloads them and recovers from a crash (`recovery`).
//! Replaced sessions are archived (`archive`).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};

use adapter_protocol::{Command as AgentCommand, Event as AgentEvent, PermissionMode as ProtocolMode};
use serde_json::Value;
use switchyard_schema::command_name;
use switchyard_schema::{
    AgentKind, ArchivedSession, Entry, Event, ModelChoice, PermissionMode, Session, SessionFacts, SessionStatus, Settings, ToolState,
    Verdict,
};

use crate::spend::{self, PriceTable, Usage};
use crate::agents::{self, AgentHandle, AgentOutput, AgentSink, LaunchSpec};
use crate::compat::{self, FailureClass};
use crate::processes::{self, Recorded};
use crate::session_store::{self, Row, CHUNK};
use crate::mcp;
use crate::policy;
use crate::secrets;
use crate::Inner;

mod archive;
mod recovery;
mod model_switch;
mod requests;
pub(crate) use archive::{archive_gone, archive_occupant, export, read_archived, replace, update_settings};
pub(crate) use model_switch::set_model;
use model_switch::{first_request, resume_on_default, send_queued_switch, Resumed};
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
    /// The newest protocol events the agent sent, as JSON, for the Agent log's debug view
    /// (at most [`RAW_EVENT_BYTES`]; older ones are dropped, the rest keep their numbers).
    raw_events: RawEvents,
    /// Transcript index of each Permission request, by request id.
    requests: HashMap<String, usize>,
    /// Every token_usage report, oldest first. Each holds running totals.
    usage: Vec<Usage>,
    /// The mode the CLI was asked to run in, in protocol terms (bypass in a sandbox).
    expected_mode: ProtocolMode,
    /// The running agent; `None` once its process is gone (exited, or a previous run of
    /// the app started it). Talking to the agent again relaunches it by `external_id`.
    agent: Option<Box<dyn AgentHandle>>,
    /// The CLI's own session id (from session_init), for resume-by-id.
    external_id: Option<String>,
    /// The agent process, recorded so a relaunch after a crash can kill it.
    process: Option<Recorded>,
    /// The pending Model switch (`session.pending_model`) is not yet sent to the agent: it
    /// waits for a Permission request's answer, or for the agent to be running again.
    switch_queued: bool,
    /// The agent was relaunched to resume, and its first request has not run yet.
    resumed: Option<Resumed>,
    /// How many times the agent was relaunched: only the latest process's output counts.
    launch: u32,
    saved: Saved,
}

/// The stored row's changing parts: status, facts, expected mode, external id, process,
/// entry count, and the Model switch (pending, last confirmed).
type RowFacts =
    (SessionStatus, SessionFacts, ProtocolMode, Option<String>, Option<Recorded>, usize, (Option<String>, Option<String>));

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
    model: ModelChoice,
    prompt: &str,
) {
    begin(inner, core, command_name::START_SESSION, worktree_path, agent, permission_mode, model, prompt, None);
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
    model: ModelChoice,
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
    let mcp_servers = match mcp::enabled(inner, &repo.id, std::path::Path::new(&repo.path)) {
        Ok(servers) => servers,
        Err(reason) => return refuse(inner, format!("Could not start the session: {reason}")),
    };
    // Before the first session, not mid-session: an unsupported CLI never starts (#13).
    if let Err(reason) = compat::admit(inner, agent) {
        return refuse(inner, reason);
    }
    let id = format!("session-{}", inner.sessions.next_id + 1);
    // Grants reach the CLI as flags; the CLI's own settings files are never written.
    let (allowed_tools, disallowed_tools) = policy::cli_grants(agent, &policy::resolve(inner, &repo));
    let sandboxed = policy::sandboxed(inner, &repo);
    let expected_mode = agents::protocol_mode(permission_mode, sandboxed);
    let spec = LaunchSpec {
        agent,
        cwd: worktree.path.clone(),
        permission_mode,
        model: model.id().map(str::to_string),
        sandboxed,
        allowed_tools,
        disallowed_tools,
        mcp_servers: mcp_servers.clone(),
        env: secrets::spawn_env(inner, &repo),
    };
    let handle = match inner.runtime.agents.launch(spec, sink(Arc::downgrade(core), id.clone(), 0)) {
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
    crate::models::remember(inner, agent, &repo.id, &model);
    let state_repo = repo.id.clone();
    let session = Session {
        id: id.clone(),
        repository_id: repo.id,
        worktree_path: worktree_path.into(),
        branch: worktree.branch,
        agent,
        permission_mode,
        model,
        switched_model: None,
        pending_model: None,
        status: SessionStatus::Running,
        started_at_ms: (inner.runtime.now_ms)(),
    };
    inner.emit(Event::SessionStarted { session: session.clone() });
    inner.sessions.list.push(SessionState {
        session,
        facts: SessionFacts { mcp_servers: mcp::facts(&mcp_servers), ..SessionFacts::default() },
        transcript: Vec::new(),
        tool_calls: HashMap::new(),
        raw_events: RawEvents::default(),
        requests: HashMap::new(),
        usage: Vec::new(),
        expected_mode,
        agent: Some(handle),
        external_id: None,
        process,
        switch_queued: false,
        resumed: None,
        launch: 0,
        saved: Saved::default(),
    });
    let state = inner.sessions.list.last_mut().expect("just pushed");
    let entry = append(state, Entry::User { text: prompt.into() });
    inner.emit(entry);
    // What the committed policy asks for but the user has not acknowledged does not apply.
    if let Some(line) = policy::repository(inner, &state_repo).and_then(|r| policy::pending_summary(inner, &r)) {
        if let Some(state) = find(inner, &id) {
            let entry = append(state, Entry::System { text: line });
            inner.emit(entry);
        }
    }
    send_to_agent(inner, &id, AgentCommand::SendText { text: prompt.into() });
    // Every Session start refreshes the cached Model list for its CLI, in the background.
    send_to_agent(inner, &id, AgentCommand::ListModels);
}

pub(crate) fn stop(inner: &mut Inner, id: &str) {
    const COMMAND: &str = command_name::STOP_SESSION;
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else {
        return inner.refuse(COMMAND, crate::no_session(&id));
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
    const COMMAND: &str = command_name::SEND_PROMPT;
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else {
        return inner.refuse(COMMAND, crate::no_session(&id));
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
    send_queued_switch(inner, id);
    send_text(inner, id, text);
}

/// The `resumeSession` command: a Paused or Finished session continues where it left off.
/// A live agent is simply told to continue; one whose process is gone is relaunched first,
/// through its CLI's resume-by-id, so it keeps its context.
pub(crate) fn resume(inner: &mut Inner, core: &Arc<Mutex<Inner>>, id: &str) {
    const COMMAND: &str = command_name::RESUME_SESSION;
    let Some(state) = find(inner, id) else {
        return inner.refuse(COMMAND, crate::no_session(&id));
    };
    if transition(state.session.status, &Cause::Resume).is_none() {
        let label = status_label(state.session.status);
        return inner.refuse(COMMAND, format!("A {label} session cannot be resumed"));
    }
    // A switch its CLI cannot take live applies on resume: a process still running is
    // replaced by one resumed on the new Model.
    let queued = state.switch_queued && state.session.pending_model.is_some() && state.agent.is_some();
    let agent = state.session.agent;
    if queued && !compat::switches_model_live(inner, agent) {
        let state = find(inner, id).expect("found above");
        state.agent = None;
        state.process = None;
    }
    if let Err(reason) = ensure_agent(inner, core, id) {
        return inner.refuse(COMMAND, reason);
    }
    let Some(state) = find(inner, id) else { return };
    let Some(status) = change_status(state, &Cause::Resume) else { return };
    let entry = append(state, Entry::System { text: "Resumed by you.".into() });
    inner.emit(status);
    inner.emit(entry);
    send_queued_switch(inner, id);
    send_text(inner, id, CONTINUE);
}

/// Send the agent `text`. A relaunched agent keeps it, to be sent again if it falls back to
/// Default.
fn send_text(inner: &mut Inner, id: &str, text: &str) {
    if let Some(resumed) = find(inner, id).and_then(|state| state.resumed.as_mut()) {
        resumed.text.get_or_insert_with(|| text.to_string());
    }
    send_to_agent(inner, id, AgentCommand::SendText { text: text.into() });
}

/// Make sure the session has a running agent: relaunch a gone one by its CLI session id.
fn ensure_agent(inner: &mut Inner, core: &Arc<Mutex<Inner>>, id: &str) -> Result<(), String> {
    let Some(state) = find(inner, id) else { return Err(crate::no_session(&id)) };
    let agent_kind = state.session.agent;
    if state.agent.is_some() {
        return Ok(());
    }
    let live = compat::switches_model_live(inner, agent_kind);
    let resumed = model_switch::resume_on(find(inner, id).expect("found above"), live);
    relaunch(inner, Arc::downgrade(core), id, resumed)
}

/// Relaunch the session's agent by its CLI session id, resuming on `resumed.model`.
fn relaunch(inner: &mut Inner, core: Weak<Mutex<Inner>>, id: &str, resumed: Resumed) -> Result<(), String> {
    let Some(state) = find(inner, id) else { return Err(crate::no_session(&id)) };
    let agent = state.session.agent;
    let Some(external_id) = state.external_id.clone() else {
        return Err(format!(
            "There is nothing to resume: {} never started for this session; start a new one",
            agent_name(agent)
        ));
    };
    let (cwd, permission_mode) = (state.session.worktree_path.clone(), state.session.permission_mode);
    let (state_model, state_launch) = (resumed.model.clone(), state.launch);
    let repository_id = state.session.repository_id.clone();
    // Grants as they are now: some may have been made or revoked since the session began.
    let repository = policy::repository(inner, &repository_id);
    // The session's own mode (as last switched); the sandbox as the policy declares it now,
    // so a sandbox withdrawn since never relaunches into bypass.
    let ((allowed_tools, disallowed_tools), sandboxed) = match &repository {
        Some(repo) => (policy::cli_grants(agent, &policy::resolve(inner, repo)), policy::sandboxed(inner, repo)),
        None => Default::default(),
    };
    // The repository's MCP servers the user has enabled, as `.mcp.json` lists them now.
    let mcp_servers = match &repository {
        Some(repo) => mcp::enabled(inner, &repo.id, std::path::Path::new(&repo.path))
            .map_err(|reason| format!("Could not resume {}: {reason}", agent_name(agent)))?,
        None => Vec::new(),
    };
    let expected_mode = agents::protocol_mode(permission_mode, sandboxed);
    let spec = LaunchSpec {
        agent,
        cwd: cwd.into(),
        permission_mode,
        model: state_model.clone(),
        sandboxed,
        allowed_tools,
        disallowed_tools,
        mcp_servers: mcp_servers.clone(),
        // Secrets as granted now: a revoke since the session began takes effect here.
        env: repository.as_ref().map(|repo| secrets::spawn_env(inner, repo)).unwrap_or_default(),
    };
    let launch = state_launch + 1;
    let mut handle = match inner.runtime.agents.launch(spec, sink(core, id.to_string(), launch)) {
        Ok(handle) => handle,
        Err(reason) => {
            compat::record(inner, agent, FailureClass::SpawnFailure, None, &reason);
            return Err(format!("Could not resume {}: {reason}", agent_name(agent)));
        }
    };
    let process = processes::record(&*inner.runtime.processes, handle.pid());
    let sent = handle.send(AgentCommand::Resume { external_session_id: external_id, model: state_model });
    // A relaunch starts the CLI again (perhaps upgraded since): refresh its Model list too.
    if sent.is_ok() {
        let _ = handle.send(AgentCommand::ListModels);
    }
    let state = find(inner, id).expect("the session is still there");
    // A process it replaces (killed as its handle drops) has nothing more to say.
    state.launch = launch;
    state.agent = Some(handle);
    state.process = process;
    state.expected_mode = expected_mode;
    // A new process: its servers' states come from its own init.
    state.facts.mcp_servers = mcp::facts(&mcp_servers);
    if resumed.applies.is_some() {
        // Sent with the resume: there is nothing left to send live.
        state.switch_queued = false;
    }
    state.resumed = Some(resumed);
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
        state.expected_mode,
        state.external_id.clone(),
        state.process.clone(),
        state.transcript.len(),
        (state.session.pending_model.clone(), state.session.switched_model.clone()),
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
                    expected_mode: Some(state.expected_mode),
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

/// The `setPermissionMode` command: the composer's mode segment. Explore is chosen only
/// at start (it is the only kind allowed on a primary worktree), and the Repository's
/// ceiling caps the switch as it caps the start.
pub(crate) fn set_mode(inner: &mut Inner, id: &str, mode: PermissionMode) {
    const COMMAND: &str = command_name::SET_PERMISSION_MODE;
    let Some(state) = find(inner, id) else {
        return inner.refuse(COMMAND, crate::no_session(&id));
    };
    let (from, repository_id, status) = (state.session.permission_mode, state.session.repository_id.clone(), state.session.status);
    if from == mode {
        return;
    }
    if from == PermissionMode::Explore {
        return inner.refuse(COMMAND, "An Explore session stays in Explore: start a new session to make changes".into());
    }
    if mode == PermissionMode::Explore {
        return inner.refuse(COMMAND, "Explore is chosen when a session starts: start an Explore session instead".into());
    }
    if status == SessionStatus::Failed {
        return inner.refuse(COMMAND, "The agent has stopped; start a new session".into());
    }
    let Some(repo) = policy::repository(inner, &repository_id) else {
        return inner.refuse(COMMAND, crate::no_repository(&repository_id));
    };
    if let Err(reason) = policy::admit_mode(&repo, mode) {
        return inner.refuse(COMMAND, reason);
    }
    let expected = agents::protocol_mode(mode, policy::sandboxed(inner, &repo));
    let Some(state) = find(inner, id) else { return };
    state.session.permission_mode = mode;
    state.expected_mode = expected;
    // A warning about the old mode no longer holds; the CLI does not re-report its mode.
    let cleared = state.facts.mode_warning.take().is_some();
    let mut events = vec![Event::SessionModeChanged { session_id: id.into(), permission_mode: mode }];
    if cleared {
        events.push(facts(state));
    }
    let text = format!("Permission mode: {} (was {}).", policy::mode_label(mode), policy::mode_label(from));
    events.push(append(state, Entry::System { text }));
    for event in events {
        inner.emit(event);
    }
    send_to_agent(inner, id, AgentCommand::SetPermissionMode { permission_mode: expected });
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

/// The id of the session occupying the worktree at `path`, if any.
pub(crate) fn occupant_id(inner: &Inner, path: &str) -> Option<String> {
    inner.sessions.list.iter().find(|s| s.session.worktree_path == path).map(|s| s.session.id.clone())
}

/// The live session `id`, as last emitted.
pub(crate) fn live(inner: &Inner, id: &str) -> Option<Session> {
    inner.sessions.list.iter().find(|s| s.session.id == id).map(|s| s.session.clone())
}

/// The ids of every live session.
pub(crate) fn live_ids(inner: &Inner) -> Vec<String> {
    inner.sessions.list.iter().map(|s| s.session.id.clone()).collect()
}

/// Every session, live or archived, that ran on `branch` in the worktree at `path`.
pub(crate) fn ids_on_branch(inner: &Inner, path: &str, branch: &str) -> Vec<String> {
    let live = inner.sessions.list.iter().map(|s| &s.session);
    let archived = inner.sessions.archived.iter().map(|a| &a.session);
    live.chain(archived)
        .filter(|s| s.worktree_path == path && s.branch.as_deref() == Some(branch))
        .map(|s| s.id.clone())
        .collect()
}

/// Is there a session `id`, live or archived?
pub(crate) fn known(inner: &Inner, id: &str) -> bool {
    inner.sessions.list.iter().any(|s| s.session.id == id) || inner.sessions.archived.iter().any(|a| a.session.id == id)
}

/// Note `text` in the transcript of every session of the repository whose agent process is
/// running (a change it will only see at its next start).
pub(crate) fn note_running(inner: &mut Inner, repository_id: &str, text: &str) {
    let mut events = Vec::new();
    for state in inner.sessions.list.iter_mut() {
        if state.session.repository_id == repository_id && state.agent.is_some() {
            events.push(append(state, Entry::System { text: text.into() }));
        }
    }
    for event in events {
        inner.emit(event);
    }
}

/// Ask one Session whose agent of `kind` is running CLI `cli_version` to list its Models (a
/// background refresh of the Model-list cache; the answer arrives as agent output).
pub(crate) fn ask_for_models(inner: &mut Inner, kind: AgentKind, cli_version: &str) {
    let live = inner.sessions.list.iter().find(|s| {
        s.session.agent == kind && s.agent.is_some() && s.facts.cli_version.as_deref() == Some(cli_version)
    });
    if let Some(id) = live.map(|s| s.session.id.clone()) {
        send_to_agent(inner, &id, AgentCommand::ListModels);
    }
}

/// Emit every session, its facts, its spend (priced now) and its transcript (on attach).
pub(crate) fn replay(inner: &Inner) {
    let table = PriceTable::read(&inner.data_dir);
    for state in &inner.sessions.list {
        let id = &state.session.id;
        inner.emit(Event::SessionStarted { session: state.session.clone() });
        inner.emit(Event::SessionFactsChanged { session_id: id.clone(), facts: state.facts.clone() });
        for upto in 0..state.usage.len() {
            inner.emit(spend_event(state, upto, &table));
        }
        for (index, entry) in state.transcript.iter().enumerate() {
            inner.emit(Event::TranscriptEntry { session_id: id.clone(), index: index as u32, entry: entry.clone() });
        }
        for (index, json) in state.raw_events.numbered() {
            inner.emit(Event::AgentEventReceived { session_id: id.clone(), index, json: json.clone() });
        }
    }
    inner.emit(Event::SettingsChanged { settings: inner.sessions.settings.clone() });
    for archived in &inner.sessions.archived {
        inner.emit(Event::SessionArchived { archived: archived.clone() });
    }
}

/// Where the session's `launch`-th agent process reports.
fn sink(core: Weak<Mutex<Inner>>, session_id: String, launch: u32) -> AgentSink {
    Arc::new(move |output| {
        if let Some(strong) = core.upgrade() {
            let mut inner = strong.lock().expect("core lock");
            // A process the session has replaced since (relaunched it) has nothing more to say.
            if find(&mut inner, &session_id).is_some_and(|s| s.launch == launch) {
                on_agent(&mut inner, &core, &session_id, output);
            }
            flush(&mut inner);
        }
    })
}

fn find<'a>(inner: &'a mut Inner, id: &str) -> Option<&'a mut SessionState> {
    inner.sessions.list.iter_mut().find(|s| s.session.id == id)
}

fn on_agent(inner: &mut Inner, core: &Weak<Mutex<Inner>>, id: &str, output: AgentOutput) {
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
    let mut model_list = None;
    let mut fall_back = None;
    let turn_ended = matches!(output, AgentOutput::Event(AgentEvent::TurnComplete | AgentEvent::Done { .. }));
    // A resumed agent's first request has run once it says or does something, or its turn ends.
    if matches!(&output, AgentOutput::Event(event) if event.shows_a_request_ran()) {
        out.extend(first_request(state));
    }
    match output {
        AgentOutput::Event(event) => match event {
            AgentEvent::SessionInit {
                external_session_id,
                permission_mode,
                cli_permission_mode,
                cli,
                model,
                mcp_servers,
            } => {
                mcp::on_init(&mut state.facts.mcp_servers, mcp_servers, agent_name(state.session.agent));
                state.facts.mode_warning = (permission_mode != state.expected_mode).then(|| {
                    format!(
                        "Asked for {}, but {} started in {} ({cli_permission_mode})",
                        agents::mode_label(state.expected_mode),
                        agent_name(state.session.agent),
                        agents::mode_label(permission_mode),
                    )
                });
                // A relaunched CLI (resume) initialises again; the start line is written once.
                let first = state.external_id.is_none();
                state.external_id = Some(external_session_id);
                if let Some(resumed) = state.resumed.as_mut() {
                    resumed.reported = model.clone();
                }
                // What the CLI says it runs on, else the Model it was started (or resumed) on;
                // a Default it does not report stays unknown, never guessed.
                let asked = match &state.resumed {
                    Some(resumed) => resumed.model.clone(),
                    None => state.session.model.id().map(str::to_string),
                };
                let model = model.or(asked);
                if first {
                    state.facts.model = model;
                    let text = start_line(&state.session.model, state.facts.model.as_deref());
                    out.push(append(state, Entry::System { text }));
                } else if let Some(model) = model {
                    if unresolved_default(state) {
                        out.extend(resolve_default(state, model, &PriceTable::read(&inner.data_dir)));
                    } else {
                        state.facts.model = Some(model);
                    }
                }
                state.facts.cli_version = Some(cli.version);
                state.facts.cli_permission_mode = Some(cli_permission_mode);
                out.push(facts(state));
            }
            AgentEvent::McpStatus { servers } => {
                mcp::on_status(&mut state.facts.mcp_servers, servers);
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
            AgentEvent::TokenUsage { model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd } => {
                let usage = Usage {
                    at_ms: (inner.runtime.now_ms)(),
                    model,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    reported_usd: cost_usd,
                };
                let named = usage.model.clone();
                state.usage.push(usage);
                let table = PriceTable::read(&inner.data_dir);
                match named {
                    // The first concrete Model name resolves an unresolved Default.
                    Some(model) if unresolved_default(state) => {
                        out.extend(resolve_default(state, model, &table));
                        out.push(facts(state));
                    }
                    _ => out.push(spend_event(state, state.usage.len() - 1, &table)),
                }
            }
            AgentEvent::ClassifierDenial { tool, detail, reason } => {
                // Counted and noted; never a Permission request, so the status never moves.
                state.facts.classifier_denials += 1;
                let why = reason.map(|r| format!(" ({r})")).unwrap_or_default();
                out.push(append(state, Entry::System { text: format!("Denied by the classifier: {tool} {detail}{why}") }));
                out.push(facts(state));
            }
            event @ (AgentEvent::ModelsListed { .. } | AgentEvent::CannotListModels { .. }) => {
                model_list = crate::models::from_agent(event, (inner.runtime.now_ms)());
            }
            AgentEvent::ModelChanged { model } => out.extend(model_switch::changed(state, model)),
            AgentEvent::ModelChangeFailed { model, reason } => {
                let (events, fall) = model_switch::change_failed(state, model, reason);
                out.extend(events);
                fall_back = fall;
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
            // A switch it was told of but never confirmed goes to the relaunched one.
            state.switch_queued = state.session.pending_model.is_some();
            state.resumed = None;
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
    // A turn is when the agent commits, edits and writes its memory: re-read where its
    // branch stands and what it remembers (watching a memory directory it just made).
    if turn_ended {
        crate::disk::watch_new_memory(inner, &worktree_path);
        let config = crate::disk::config_moved(inner, &worktree_path);
        let kinds = crate::disk::Kinds { files: true, git: true, config };
        crate::disk::worktree_changed(inner, &worktree_path, kinds);
    }
    if let Some((class, detail)) = failure {
        compat::record(inner, agent, class, cli_version.as_deref(), &detail);
    }
    if let Some(list) = model_list {
        crate::models::answered(inner, agent, cli_version.as_deref(), list);
    }
    if let Some(fall_back) = fall_back {
        resume_on_default(inner, core, id, fall_back);
    }
}

/// How much raw protocol JSON a session keeps for the debug view (as a shell keeps its
/// scrollback).
pub(crate) const RAW_EVENT_BYTES: usize = 256 * 1024;

/// The newest raw events, numbered from the session's first.
#[derive(Default)]
struct RawEvents {
    /// The number of `kept[0]`.
    first: u32,
    kept: std::collections::VecDeque<String>,
    bytes: usize,
}

impl RawEvents {
    /// Keep `json`, dropping the oldest beyond the budget; returns its number.
    fn push(&mut self, json: String) -> u32 {
        let index = self.first + self.kept.len() as u32;
        self.bytes += json.len();
        self.kept.push_back(json);
        while self.bytes > RAW_EVENT_BYTES && self.kept.len() > 1 {
            let dropped = self.kept.pop_front().expect("not empty");
            self.bytes -= dropped.len();
            self.first += 1;
        }
        index
    }

    fn numbered(&self) -> impl Iterator<Item = (u32, &String)> {
        self.kept.iter().enumerate().map(|(i, json)| (self.first + i as u32, json))
    }
}

fn record_raw(state: &mut SessionState, event: &AgentEvent) -> Event {
    let json = serde_json::to_string(event).unwrap_or_else(|e| format!("{{\"unserialisable\":\"{e}\"}}"));
    let index = state.raw_events.push(json.clone());
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

/// The Session's spend as of its usage report number `upto`, priced now.
fn spend_event(state: &SessionState, upto: usize, table: &PriceTable) -> Event {
    let reports = &state.usage[..=upto];
    let spend = spend::spend(reports, state.facts.model.as_deref(), table);
    Event::SpendRecorded { session_id: state.session.id.clone(), at_ms: reports[upto].at_ms, spend }
}

/// The Agent log's line for the Model a Session started on: the chosen Model, or what
/// Default resolved to ("Model: Default → claude-opus-5-5"), or that the CLI did not say.
fn start_line(choice: &ModelChoice, running: Option<&str>) -> String {
    match (choice, running) {
        (ModelChoice::Default, Some(model)) => format!("Model: Default → {model}"),
        (ModelChoice::Default, None) => "Model: Default (not reported)".into(),
        (ModelChoice::Model { id }, running) => format!("Model: {}", running.unwrap_or(id)),
    }
}

/// Started on Default, and the CLI has not yet named the Model Default runs on: usage
/// meanwhile is unpriced.
fn unresolved_default(state: &SessionState) -> bool {
    state.session.model == ModelChoice::Default && state.facts.model.is_none()
}

/// Default resolves to `model`, the first concrete Model name the CLI gave: noted in the
/// Agent log, and the usage held unpriced until now priced on it, once, here. The caller
/// reports the facts.
fn resolve_default(state: &mut SessionState, model: String, table: &PriceTable) -> Vec<Event> {
    let text = format!("Model: Default resolved to {model}; earlier usage is priced on it");
    state.facts.model = Some(model);
    let mut out = vec![append(state, Entry::System { text })];
    if let Some(last) = state.usage.len().checked_sub(1) {
        out.push(spend_event(state, last, table));
    }
    out
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
        AgentKind::Codex => "Codex",
    }
}

/// One line describing a tool call: the command, the path, the pattern...
fn summarise(tool: &str, input: &Value) -> String {
    let field = |name: &str| input.get(name).and_then(Value::as_str).map(str::to_owned);
    let summary = match tool {
        "Bash" | "Shell" => field("command"),
        "ApplyPatch" => input.get("paths").and_then(Value::as_array).map(|paths| {
            paths.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
        }),
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
