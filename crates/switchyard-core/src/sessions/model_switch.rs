//! A running Session's Model switch: the `setModel` command, the switch pending until the
//! agent confirms it (`model_changed`) or it fails (`model_change_failed`), a switch queued
//! behind a Permission request or a gone agent, and a resume that applies the switch (on a
//! CLI that cannot switch live) or falls back to Default (on a Model the CLI cannot run).
//!
//! `session.pending_model` is the switch asked for and not yet confirmed or failed;
//! `session.switched_model` the last confirmed one; `facts.model` the Model running. Only the
//! user's switches change the Session's Model: the agent never does.

use std::sync::{Mutex, Weak};

use adapter_protocol::Command as AgentCommand;
use switchyard_schema::command_name;
use switchyard_schema::{Entry, Event};

use super::{
    append, change_status, facts, find, relaunch, requests, resolve_default, send_text, send_to_agent, unresolved_default, Cause,
    SessionState,
};
use crate::compat;
use crate::models::{self, Names};
use crate::spend::PriceTable;
use crate::Inner;

/// A relaunched agent, until its first request has run.
pub(super) struct Resumed {
    /// The Model it resumes on; `None`: Default.
    pub model: Option<String>,
    /// Set when the resume applies the pending switch.
    pub applies: Option<AppliedSwitch>,
    /// The Model the resumed CLI says (at init) it runs on.
    pub reported: Option<String>,
    /// What it was sent to go on with (the prompt, or "continue"), sent again when it
    /// cannot run on its Model and falls back to Default.
    pub text: Option<String>,
}

impl Resumed {
    pub fn on(model: Option<String>) -> Self {
        Resumed { model, applies: None, reported: None, text: None }
    }
}

/// The pending switch a resume applies, and the Model the Session ran on before it (`None`:
/// not known), which the first request must have left for the switch to count as taken.
pub(super) struct AppliedSwitch {
    pub from: Option<String>,
}

/// The `setModel` command: the composer's Model control. The switch is pending from now
/// until the agent's own requests run on the new Model (`model_changed`) or it fails.
pub(crate) fn set_model(inner: &mut Inner, id: &str, model: &str) {
    const COMMAND: &str = command_name::SET_MODEL;
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.id == id) else {
        return inner.refuse(COMMAND, crate::no_session(&id));
    };
    let model = model.trim();
    if model.is_empty() {
        return inner.refuse(COMMAND, "Choose a Model to switch to".into());
    }
    let names = models::names(inner.store.db(), state.session.agent, state.facts.cli_version.as_deref());
    // An alias of the Model running (`opus` for claude-opus-5-5) is that Model.
    let running = state.facts.model.clone().filter(|running| names.same(running, model));
    if state.session.pending_model.as_deref() == Some(model) || (state.session.pending_model.is_none() && running.is_some()) {
        return;
    }
    // Back to the Model it runs on: the pending switch is withdrawn, and a CLI already told
    // to switch is told to stay. There is nothing to confirm.
    if let Some(running) = running {
        state.session.pending_model = None;
        let sent = !std::mem::take(&mut state.switch_queued);
        let agent = state.session.agent;
        let event = switch_event(state);
        inner.emit(event);
        if sent && compat::switches_model_live(inner, agent) {
            send_to_agent(inner, id, AgentCommand::SetModel { model: running });
        }
        return;
    }
    state.session.pending_model = Some(model.into());
    // While a Permission request is pending the switch waits for the answer: the request
    // stays exactly as asked, and the switch's one-request check starts after it.
    state.switch_queued = true;
    let event = switch_event(state);
    inner.emit(event);
    if !requests::has_pending(find(inner, id).expect("found above")) {
        send_queued_switch(inner, id);
    }
}

/// Send the agent the pending Model switch that waited (for an answer, or for the agent).
pub(super) fn send_queued_switch(inner: &mut Inner, id: &str) {
    let Some(state) = find(inner, id) else { return };
    if !state.switch_queued || state.agent.is_none() {
        return;
    }
    // A CLI that cannot switch live takes it when the Session resumes, never before.
    let agent = state.session.agent;
    if !compat::switches_model_live(inner, agent) {
        return;
    }
    let state = find(inner, id).expect("found above");
    state.switch_queued = false;
    if let Some(model) = state.session.pending_model.clone() {
        send_to_agent(inner, id, AgentCommand::SetModel { model });
    }
}

/// The Session's Model switch as it now stands.
pub(super) fn switch_event(state: &SessionState) -> Event {
    Event::SessionModelChanged {
        session_id: state.session.id.clone(),
        pending_model: state.session.pending_model.clone(),
        switched_model: state.session.switched_model.clone(),
    }
}

/// The Model a relaunched agent resumes on, the Session's current one: the last confirmed
/// switch, else the pending switch, else the start choice (`None`: Default). Applying the
/// pending switch, it says so, with the Model it is checked against. On a CLI that cannot
/// switch `live` a switch not yet sent comes first: resuming is the only way it applies.
pub(super) fn resume_on(state: &SessionState, live: bool) -> Resumed {
    let session = &state.session;
    let applying = |model: &String| Resumed {
        applies: Some(AppliedSwitch { from: state.facts.model.clone() }),
        ..Resumed::on(Some(model.clone()))
    };
    if let (false, true, Some(model)) = (live, state.switch_queued, &session.pending_model) {
        return applying(model);
    }
    if let Some(model) = &session.switched_model {
        return Resumed::on(Some(model.clone()));
    }
    if let Some(model) = &session.pending_model {
        return applying(model);
    }
    Resumed::on(session.model.id().map(str::to_string))
}

/// The resumed agent's first request has run. A switch the resume applied took when the
/// CLI reported a Model other than the one the Session ran on before; one still on that
/// Model did not. A CLI that reported no Model leaves the switch pending.
pub(super) fn first_request(state: &mut SessionState, names: &Names) -> Vec<Event> {
    let Some(resumed) = state.resumed.take() else { return Vec::new() };
    let (Some(applied), Some(reported), Some(model)) = (resumed.applies, resumed.reported, resumed.model) else {
        return Vec::new();
    };
    // Confirmed already (model_changed), withdrawn or replaced by another switch since.
    if state.switch_queued || state.session.pending_model.as_deref() != Some(model.as_str()) {
        return Vec::new();
    }
    state.session.pending_model = None;
    if applied.from.as_deref().is_some_and(|from| names.same(from, &reported)) {
        let reason = still_on(&reported);
        let line = append(state, Entry::System { text: not_taken(&model, &reason) });
        let id = state.session.id.clone();
        return vec![line, switch_event(state), Event::ModelSwitchFailed { session_id: id, model, reason: Some(reason) }];
    }
    state.session.switched_model = Some(reported.clone());
    vec![switch_event(state), append(state, Entry::System { text: format!("Model → {reported}") })]
}

/// Why a switch did not take when the agent's next request was still on `running`.
fn still_on(running: &str) -> String {
    format!("the agent's next request was still on {running}")
}

/// The Agent log's line for a switch to `model` that did not take, for `reason`.
fn not_taken(model: &str, reason: &str) -> String {
    format!("Model switch to {model} did not take: {reason}")
}

/// `model_changed`: the main agent's own requests run on `model` now. Only the user's
/// switches change the Session's Model: `model` confirms the pending switch when it names
/// that Model (by its id, or the full id an alias stands for), sent or still queued.
/// Anything else is the agent moving itself: noted, never recorded as a switch, and the
/// Session's Model stays as it was (a resume goes back to it). A switch that was sent and
/// whose next request ran on yet another Model did not take. On a Default the CLI did not
/// name at init, the first one resolves Default ([`resolve_default`]).
pub(super) fn changed(state: &mut SessionState, model: String, names: &Names, table: &PriceTable) -> Vec<Event> {
    let mut out = Vec::new();
    if state.session.pending_model.as_deref().is_some_and(|pending| names.same(pending, &model)) {
        state.session.pending_model = None;
        state.switch_queued = false;
        state.session.switched_model = Some(model.clone());
        out.push(switch_event(state));
        out.push(append(state, Entry::System { text: format!("Model → {model}") }));
        state.facts.model = Some(model);
        out.push(facts(state));
        return out;
    }
    // A Default the CLI did not name at init: the main agent's first request says what it is.
    let resolves = unresolved_default(state);
    if resolves {
        out.extend(resolve_default(state, model.clone(), table));
        out.push(facts(state));
    }
    if let (false, Some(pending)) = (state.switch_queued, state.session.pending_model.take()) {
        let reason = format!("the agent's next request ran on {model}");
        out.push(append(state, Entry::System { text: not_taken(&pending, &reason) }));
        out.push(switch_event(state));
        out.push(Event::ModelSwitchFailed { session_id: state.session.id.clone(), model: pending, reason: Some(reason) });
        return out;
    }
    if resolves {
        return out;
    }
    let text = match state.facts.model.as_deref() {
        Some(running) if names.same(running, &model) => format!("The agent is back on {running}"),
        running => {
            let running = running.unwrap_or("the Model it was on");
            format!("The agent moved itself to {model}: not your switch, so the Session's Model stays {running}")
        }
    };
    out.push(append(state, Entry::System { text }));
    out
}

/// `model_change_failed`. For the Model a relaunched agent resumed on (before its first
/// request ran), the CLI cannot run on it (retired, or no access): the Session resumes once
/// more, on Default, so it always runs, and the user picks again; the text it was sent is
/// returned, to be sent again ([`resume_on_default`]). Otherwise a switch did not take:
/// noted, never retried, and the Session carries on as it was, in the same status.
pub(super) fn change_failed(state: &mut SessionState, model: String, reason: Option<String>) -> (Vec<Event>, Option<FallBack>) {
    let id = state.session.id.clone();
    let mut out = Vec::new();
    if state.resumed.as_ref().is_some_and(|r| r.model.as_deref() == Some(model.as_str())) {
        let resumed = state.resumed.take().expect("matched above");
        let why = match &reason {
            Some(reason) if reason.ends_with('.') => format!(": {reason}"),
            Some(reason) => format!(": {reason}."),
            None => ".".into(),
        };
        out.push(append(state, Entry::System { text: format!("Could not resume on {model}{why} Resumed on Default.") }));
        let session = &mut state.session;
        let pending = session.pending_model.take_if(|m| *m == model).is_some();
        let switched = session.switched_model.take_if(|m| *m == model).is_some();
        if pending || switched {
            out.push(switch_event(state));
        }
        out.push(Event::ModelSwitchFailed { session_id: id, model, reason });
        // Unknown until the CLI resumed on Default says.
        state.facts.model = None;
        out.push(facts(state));
        return (out, Some(FallBack { text: resumed.text }));
    }
    let running = state.facts.model.as_deref().unwrap_or("the Model it was on");
    let (text, reason) = match reason {
        Some(reason) => (format!("Model switch to {model} failed: {reason}"), reason),
        None => {
            let reason = still_on(running);
            (not_taken(&model, &reason), reason)
        }
    };
    let reason = Some(reason);
    out.push(append(state, Entry::System { text }));
    // A failure for a switch since superseded by another leaves that one pending.
    if !state.switch_queued && state.session.pending_model.as_deref() == Some(model.as_str()) {
        state.session.pending_model = None;
        out.push(switch_event(state));
        out.push(Event::ModelSwitchFailed { session_id: id, model, reason });
    }
    (out, None)
}

/// A resume on a Model the CLI cannot run on falls back to Default, sending `text` again.
pub(super) struct FallBack {
    pub text: Option<String>,
}

/// Replace the agent that could not run on the Model it resumed on with one resumed on
/// Default, and send it what the first one was sent. Tried once: a resume on Default is never
/// retried.
pub(super) fn resume_on_default(inner: &mut Inner, core: &Weak<Mutex<Inner>>, id: &str, FallBack { text }: FallBack) {
    let Some(state) = find(inner, id) else { return };
    state.agent = None; // its process is killed as the handle drops
    state.process = None;
    match relaunch(inner, core.clone(), id, Resumed::on(None)) {
        Ok(()) => {
            if let Some(text) = text {
                send_text(inner, id, &text);
            }
        }
        Err(reason) => {
            let Some(state) = find(inner, id) else { return };
            let mut events = vec![append(state, Entry::System { text: reason })];
            events.extend(change_status(state, &Cause::AgentFailed));
            for event in events {
                inner.emit(event);
            }
        }
    }
}
