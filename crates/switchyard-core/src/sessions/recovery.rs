//! Reloading sessions when the core opens, and recovering from a quit or crash: every
//! recorded agent process still running (same pid, same start time) is killed, never
//! adopted; every pending Permission request is cancelled with a system line; Running and
//! Needs you sessions come back Paused. Nothing resumes on its own.

use std::collections::HashMap;

use switchyard_schema::{ArchivedSession, Entry};

use super::{append, archive, change_status, persist, requests, Cause, Saved, SessionState};
use crate::agents;
use crate::policy;
use crate::processes;
use crate::session_store;
use crate::Inner;

/// Load every stored session into `inner`, recover the live ones and apply retention.
pub(crate) fn load(inner: &mut Inner) -> rusqlite::Result<()> {
    let db = inner.store.db();
    inner.sessions.settings = session_store::settings(db);
    for row in session_store::rows(db)? {
        let seq = row.session.id.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        inner.sessions.next_id = inner.sessions.next_id.max(seq);
        if let Some(archived_at_ms) = row.archived_at_ms {
            inner.sessions.archived.push(ArchivedSession {
                session: row.session,
                archived_at_ms,
                entries: row.entries as u32,
            });
            continue;
        }
        let transcript = session_store::transcript(db, &row.session.id)?;
        let usage = session_store::usage(db, &row.session.id)?;
        let mut tool_calls = HashMap::new();
        let mut requests = HashMap::new();
        for (index, entry) in transcript.iter().enumerate() {
            match entry {
                Entry::Tool { call_id, .. } => {
                    tool_calls.insert(call_id.clone(), index);
                }
                Entry::Request { request_id, .. } => {
                    requests.insert(request_id.clone(), index);
                }
                _ => {}
            }
        }
        // Rows stored before the expected mode was kept: what the session's mode asks for
        // under the policy as it is now.
        let expected_mode = row.expected_mode.unwrap_or_else(|| {
            let sandboxed = policy::repository(inner, &row.session.repository_id).is_some_and(|r| policy::sandboxed(&r));
            agents::protocol_mode(row.session.permission_mode, sandboxed)
        });
        let saved = Saved {
            row: Some((row.session.status, row.facts.clone(), expected_mode, row.external_id.clone(), row.process.clone(), transcript.len())),
            entries: transcript.len(),
            chunks: Default::default(),
            usage: usage.len(),
        };
        let mut state = SessionState {
            session: row.session,
            facts: row.facts,
            transcript,
            tool_calls,
            raw_events: Default::default(),
            requests,
            usage,
            expected_mode,
            agent: None,
            external_id: row.external_id,
            process: row.process,
            saved,
        };
        recover(inner, &mut state);
        let _ = persist(inner.store.db(), &mut state, None);
        inner.sessions.list.push(state);
    }
    archive::prune(inner);
    Ok(())
}

/// The app is starting: no agent of this session runs for us any more.
fn recover(inner: &Inner, state: &mut SessionState) {
    if let Some(process) = state.process.take() {
        processes::kill_if_same(&*inner.runtime.processes, &process);
    }
    let cancelled = requests::cancel_all(state);
    if !cancelled.is_empty() {
        let text = if cancelled.len() == 1 {
            "Switchyard closed while a request waited for you; it was cancelled.".to_string()
        } else {
            format!("Switchyard closed while {} requests waited for you; they were cancelled.", cancelled.len())
        };
        append(state, Entry::System { text });
    }
    if change_status(state, &Cause::Recovered).is_some() {
        append(
            state,
            Entry::System { text: "Paused: Switchyard closed while the agent was working. Resume to continue.".into() },
        );
    }
}
