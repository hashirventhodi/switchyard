//! The permission relay: an agent's Permission request becomes a request card in the
//! transcript and puts its Session in Waiting; the first answer from any surface resolves
//! it and goes back to the agent; a Session stays Waiting while any request is pending.
//! The PolicyEngine is consulted first: a request a Grant or denial already answers never
//! surfaces, it is answered at once and noted as a system line.

use adapter_protocol::{Command as AgentCommand, RequestDetail, Verdict as AgentVerdict};
use switchyard_schema::{Entry, Event, PermissionMode, Request, RequestState, SessionStatus, Verdict};

use super::{append, change_status, send_to_agent, Cause, SessionState};
use crate::policy::{self, Decision};
use crate::Inner;

/// What the agent is told when a request arrives on a session the user has stopped.
const STOPPED: &str = "The session was stopped.";

/// A request from the agent: a pending card, and the session needs the user.
pub(super) fn arrived(inner: &mut Inner, id: &str, request_id: String, detail: RequestDetail, reason: Option<String>) {
    let Some(state) = super::find(inner, id) else { return };
    let live = matches!(state.session.status, SessionStatus::Running | SessionStatus::Waiting);
    let (repository_id, mode) = (state.session.repository_id.clone(), state.session.permission_mode);
    if live && answered_by_policy(inner, id, &repository_id, mode, &request_id, &detail) {
        return;
    }
    let Some(state) = super::find(inner, id) else { return };
    let entry = Entry::Request {
        request_id: request_id.clone(),
        request: request(detail),
        reason,
        state: if live { RequestState::Pending } else { RequestState::Cancelled },
        answer: None,
    };
    state.requests.insert(request_id.clone(), state.transcript.len());
    let mut events = vec![append(state, entry)];
    if live {
        events.extend(change_status(state, &Cause::NeedsUser));
    }
    for event in events {
        inner.emit(event);
    }
    if !live {
        // Stopped (or over): nobody will answer it, so the agent must not wait on it.
        let verdict = AgentVerdict::Deny { message: Some(STOPPED.into()) };
        send_to_agent(inner, id, AgentCommand::AnswerRequest { request_id, verdict });
    }
}

/// Answer the request from policy, when a Grant or denial covers it. True when answered.
fn answered_by_policy(
    inner: &mut Inner,
    id: &str,
    repository_id: &str,
    mode: PermissionMode,
    request_id: &str,
    detail: &RequestDetail,
) -> bool {
    let Some(repo) = policy::repository(inner, repository_id) else { return false };
    let (line, verdict) = match policy::decide(&policy::resolve(inner, &repo), detail, mode) {
        Decision::Ask => return false,
        Decision::Allow(grant) => (
            format!("Allowed without asking: {} ({} {})", asked(detail), policy::source(&grant), policy::describe(&grant.rule)),
            AgentVerdict::AllowOnce,
        ),
        Decision::Deny(grant) => {
            let rule = policy::describe(&grant.rule);
            let who = policy::source(&grant);
            (
                format!("Denied without asking: {} ({who} denies {rule})", asked(detail)),
                AgentVerdict::Deny { message: Some(format!("Denied by the {who} in Switchyard: {rule}")) },
            )
        }
    };
    let Some(state) = super::find(inner, id) else { return false };
    let entry = append(state, Entry::System { text: line });
    inner.emit(entry);
    send_to_agent(inner, id, AgentCommand::AnswerRequest { request_id: request_id.into(), verdict });
    true
}

/// The thing asked for, as a system line names it.
fn asked(detail: &RequestDetail) -> String {
    match detail {
        RequestDetail::Command { command, .. } => command.clone(),
        RequestDetail::Network { domain, .. } => domain.clone(),
        RequestDetail::McpTool { server, tool, .. } => format!("{server} · {tool}"),
        RequestDetail::FileWrite { path, .. } => path.clone(),
        RequestDetail::Question { question, .. } => question.clone(),
    }
}

/// The `answerRequest` command.
pub(crate) fn answer(inner: &mut Inner, id: &str, request_id: &str, verdict: Verdict) {
    const COMMAND: &str = "answerRequest";
    let Some(state) = super::find(inner, id) else {
        return inner.refuse(COMMAND, format!("There is no session {id}"));
    };
    let Some(&index) = state.requests.get(request_id) else {
        return inner.refuse(COMMAND, format!("There is no request {request_id} in this session"));
    };
    let Entry::Request { request, state: request_state, .. } = &state.transcript[index] else { return };
    if *request_state != RequestState::Pending {
        return inner.refuse(COMMAND, "That request was already answered".into());
    }
    let question = matches!(request, Request::Question { .. });
    match (&verdict, question) {
        (Verdict::Answer { .. }, false) => return inner.refuse(COMMAND, "Only a question takes an answer".into()),
        (Verdict::AllowOnce, true) => return inner.refuse(COMMAND, "A question needs an answer".into()),
        _ => {}
    }
    if let Verdict::AllowAlways { pattern } = &verdict {
        let (detail, repository_id) = (detail(request), state.session.repository_id.clone());
        let rule = match policy::grant_for(&detail, pattern.as_deref()) {
            Ok(rule) => rule,
            Err(reason) => return inner.refuse(COMMAND, reason),
        };
        let Some(repo) = policy::repository(inner, &repository_id) else {
            return inner.refuse(COMMAND, format!("There is no repository {repository_id}"));
        };
        if let Err(reason) = policy::add_local(inner, &repo.id, &rule) {
            return inner.refuse(COMMAND, reason);
        }
        resolve(inner, id, request_id, Verdict::AllowAlways { pattern: Some(policy::describe(&rule)) });
        return policy::emit(inner, &repo);
    }
    resolve(inner, id, request_id, verdict);
}

/// The first pending question, which a prompt typed in the composer answers.
pub(super) fn pending_question(state: &SessionState) -> Option<String> {
    pending(state)
        .find(|(_, entry)| matches!(entry, Entry::Request { request: Request::Question { .. }, .. }))
        .map(|(request_id, _)| request_id)
}

pub(super) fn has_pending(state: &SessionState) -> bool {
    pending(state).next().is_some()
}

/// Mark every pending request cancelled (the session was stopped). The CLI drops them
/// itself when its turn is interrupted.
pub(super) fn cancel_all(state: &mut SessionState) -> Vec<Event> {
    let ids: Vec<String> = pending(state).map(|(request_id, _)| request_id).collect();
    ids.iter().filter_map(|request_id| settle(state, request_id, RequestState::Cancelled, None)).collect()
}

/// Record the verdict on the card, move the session on when nothing else is pending, and
/// tell the agent.
pub(super) fn resolve(inner: &mut Inner, id: &str, request_id: &str, verdict: Verdict) {
    let Some(state) = super::find(inner, id) else { return };
    let (settled, answer, agent_verdict) = match verdict {
        Verdict::AllowOnce => (RequestState::Allowed, None, AgentVerdict::AllowOnce),
        Verdict::AllowAlways { pattern } => (RequestState::AlwaysAllowed, pattern, AgentVerdict::AllowAlways),
        Verdict::Deny => (RequestState::Denied, None, AgentVerdict::Deny { message: None }),
        Verdict::Answer { text } => (RequestState::Answered, Some(text.clone()), AgentVerdict::Answer { text }),
    };
    let mut events: Vec<Event> = settle(state, request_id, settled, answer).into_iter().collect();
    if !has_pending(state) {
        events.extend(change_status(state, &Cause::RequestAnswered));
    }
    for event in events {
        inner.emit(event);
    }
    send_to_agent(inner, id, AgentCommand::AnswerRequest { request_id: request_id.into(), verdict: agent_verdict });
}

fn settle(state: &mut SessionState, request_id: &str, to: RequestState, given: Option<String>) -> Option<Event> {
    let index = *state.requests.get(request_id)?;
    let Entry::Request { state: request_state, answer, .. } = &mut state.transcript[index] else { return None };
    *request_state = to;
    *answer = given;
    Some(Event::TranscriptEntry {
        session_id: state.session.id.clone(),
        index: index as u32,
        entry: state.transcript[index].clone(),
    })
}

/// Pending requests in transcript order.
fn pending(state: &SessionState) -> impl Iterator<Item = (String, &Entry)> {
    let mut indexed: Vec<(&String, &usize)> = state.requests.iter().collect();
    indexed.sort_by_key(|(_, index)| **index);
    indexed.into_iter().filter_map(|(request_id, &index)| match &state.transcript[index] {
        entry @ Entry::Request { state: RequestState::Pending, .. } => Some((request_id.clone(), entry)),
        _ => None,
    })
}

/// A request card back in protocol terms, for the PolicyEngine.
fn detail(request: &Request) -> RequestDetail {
    match request.clone() {
        Request::Command { command, cwd } => RequestDetail::Command { command, cwd },
        Request::FileWrite { path, diff } => RequestDetail::FileWrite { path, diff },
        Request::Network { domain, url } => RequestDetail::Network { domain, url },
        Request::McpTool { server, tool, input } => {
            RequestDetail::McpTool { server, tool, input: serde_json::from_str(&input).unwrap_or_default() }
        }
        Request::Question { question, options } => RequestDetail::Question { question, options },
    }
}

fn request(detail: RequestDetail) -> Request {
    match detail {
        RequestDetail::Command { command, cwd } => Request::Command { command, cwd },
        RequestDetail::FileWrite { path, diff } => Request::FileWrite { path, diff },
        RequestDetail::Network { domain, url } => Request::Network { domain, url },
        RequestDetail::McpTool { server, tool, input } => Request::McpTool { server, tool, input: input.to_string() },
        RequestDetail::Question { question, options } => Request::Question { question, options },
    }
}
