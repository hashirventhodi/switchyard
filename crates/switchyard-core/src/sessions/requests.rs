//! The permission relay: an agent's Permission request becomes a request card in the
//! transcript and puts its Session in Waiting; the first answer from any surface resolves
//! it and goes back to the agent; a Session stays Waiting while any request is pending.
//! Policy (answering without asking) arrives with Grants; until then every request asks.

use adapter_protocol::{Command as AgentCommand, RequestDetail, Verdict as AgentVerdict};
use switchyard_schema::{Entry, Event, Request, RequestState, SessionStatus, Verdict};

use super::{append, change_status, send_to_agent, Cause, SessionState};
use crate::Inner;

/// What the agent is told when a request arrives on a session the user has stopped.
const STOPPED: &str = "The session was stopped.";

/// A request from the agent: a pending card, and the session needs the user.
pub(super) fn arrived(inner: &mut Inner, id: &str, request_id: String, detail: RequestDetail, reason: Option<String>) {
    let Some(state) = super::find(inner, id) else { return };
    let live = matches!(state.session.status, SessionStatus::Running | SessionStatus::Waiting);
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

fn request(detail: RequestDetail) -> Request {
    match detail {
        RequestDetail::Command { command, cwd } => Request::Command { command, cwd },
        RequestDetail::FileWrite { path, diff } => Request::FileWrite { path, diff },
        RequestDetail::Network { domain, url } => Request::Network { domain, url },
        RequestDetail::McpTool { server, tool, input } => Request::McpTool { server, tool, input: input.to_string() },
        RequestDetail::Question { question, options } => Request::Question { question, options },
    }
}
