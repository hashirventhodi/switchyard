//! Archived sessions: a Session that was replaced ("stop and replace") or whose worktree
//! was removed. Its transcript stays in the store as the audit trail, keyed by repository,
//! branch and session id, so it outlives the worktree. It can be read and exported, never
//! resumed. Retention (user settings, default 50 per repository and 30 days) prunes the
//! oldest; both limits apply.

use std::io::Write;
use std::sync::{Arc, Mutex};

use switchyard_schema::command_name;
use switchyard_schema::{AgentKind, ArchivedSession, Entry, Event, PermissionMode, SessionStatus, Settings};

use super::{append, begin, change_status, persist, requests, status_label, Cause};
use crate::processes;
use crate::session_store;
use crate::Inner;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// The `replaceSession` command: archive a Paused, Finished or Error session and start a
/// new one on its worktree.
pub(crate) fn replace(
    inner: &mut Inner,
    core: &Arc<Mutex<Inner>>,
    id: &str,
    agent: AgentKind,
    permission_mode: PermissionMode,
    prompt: &str,
) {
    const COMMAND: &str = command_name::REPLACE_SESSION;
    let Some(state) = super::find(inner, id) else {
        return inner.refuse(COMMAND, crate::no_session(&id));
    };
    if !replaceable(state.session.status) {
        let label = status_label(state.session.status);
        return inner.refuse(COMMAND, format!("A {label} session is never replaced; stop it first"));
    }
    let worktree_path = state.session.worktree_path.clone();
    begin(inner, core, COMMAND, &worktree_path, agent, permission_mode, prompt, Some(id));
}

/// Paused, Finished and Error sessions may be replaced or archived; an agent at work never is.
fn replaceable(status: SessionStatus) -> bool {
    matches!(status, SessionStatus::Idle | SessionStatus::Done | SessionStatus::Failed)
}

/// Archive the session on the worktree at `path` (it is being removed). Returns `Err` with
/// the reason when the session is still at work.
pub(crate) fn archive_occupant(inner: &mut Inner, path: &str) -> Result<(), String> {
    let Some(state) = inner.sessions.list.iter().find(|s| s.session.worktree_path == path) else { return Ok(()) };
    if !replaceable(state.session.status) {
        return Err(status_label(state.session.status).to_string());
    }
    let id = state.session.id.clone();
    archive(inner, &id);
    Ok(())
}

/// Archive the session on the worktree at `path`, which git has already removed (outside
/// Switchyard). An agent at work is stopped first; its transcript says why.
pub(crate) fn archive_gone(inner: &mut Inner, path: &str) {
    let Some(state) = inner.sessions.list.iter_mut().find(|s| s.session.worktree_path == path) else { return };
    let id = state.session.id.clone();
    if let Some(status) = change_status(state, &Cause::UserStop) {
        let entry = append(state, Entry::System { text: "Stopped: its worktree was removed outside Switchyard.".into() });
        let cancelled = requests::cancel_all(state);
        inner.emit(status);
        inner.emit(entry);
        for event in cancelled {
            inner.emit(event);
        }
    }
    archive(inner, &id);
}

/// Archive the session `id`: stop its agent, keep its transcript, drop it from the live
/// sessions, then apply retention.
pub(super) fn archive(inner: &mut Inner, id: &str) {
    let Some(at) = inner.sessions.list.iter().position(|s| s.session.id == id) else { return };
    let mut state = inner.sessions.list.remove(at);
    if let Some(process) = state.process.take() {
        processes::kill_if_same(&*inner.runtime.processes, &process);
    }
    state.agent = None; // dropping the handle ends the agent's transport
    let archived_at_ms = (inner.runtime.now_ms)();
    let _ = persist(inner.store.db(), &mut state, Some(archived_at_ms));
    let archived = ArchivedSession { session: state.session, archived_at_ms, entries: state.transcript.len() as u32 };
    inner.sessions.archived.push(archived.clone());
    inner.emit(Event::SessionArchived { archived });
    prune(inner);
}

/// Forget archived sessions beyond the newest `archive_keep_per_repository` of their
/// repository or older than `archive_keep_days`.
pub(super) fn prune(inner: &mut Inner) {
    let Settings { archive_keep_per_repository: keep, archive_keep_days: days } = inner.sessions.settings.clone();
    let now = (inner.runtime.now_ms)();
    let max_age = days as u64 * DAY_MS;
    let list = &inner.sessions.archived;
    let mut doomed = Vec::new();
    for (i, archived) in list.iter().enumerate() {
        let repo = &archived.session.repository_id;
        let newer = list[i + 1..].iter().filter(|a| a.session.repository_id == *repo).count();
        let too_old = now.saturating_sub(archived.archived_at_ms) > max_age;
        if newer >= keep as usize || too_old {
            doomed.push(i);
        }
    }
    let mut gone = Vec::new();
    for i in doomed.into_iter().rev() {
        let archived = inner.sessions.archived.remove(i);
        let _ = session_store::delete(inner.store.db(), &archived.session.id);
        gone.push(archived.session);
    }
    for session in gone.into_iter().rev() {
        inner.emit(Event::ArchivedSessionPruned { repository_id: session.repository_id, session_id: session.id });
    }
}

/// The `readArchivedSession` command.
pub(crate) fn read_archived(inner: &Inner, id: &str) {
    const COMMAND: &str = command_name::READ_ARCHIVED_SESSION;
    if !inner.sessions.archived.iter().any(|a| a.session.id == id) {
        return inner.refuse(COMMAND, format!("There is no archived session {id}"));
    }
    match session_store::transcript(inner.store.db(), id) {
        Ok(entries) => inner.emit(Event::ArchivedTranscript { session_id: id.into(), entries }),
        Err(e) => inner.refuse(COMMAND, format!("Could not read the archived transcript: {e}")),
    }
}

/// The `exportTranscript` command: one JSON object per transcript entry, one per line.
pub(crate) fn export(inner: &Inner, id: &str) {
    const COMMAND: &str = command_name::EXPORT_TRANSCRIPT;
    let live = inner.sessions.list.iter().find(|s| s.session.id == id);
    let (branch, entries) = match live {
        Some(state) => (state.session.branch.clone(), state.transcript.clone()),
        None => match inner.sessions.archived.iter().find(|a| a.session.id == id) {
            Some(archived) => match session_store::transcript(inner.store.db(), id) {
                Ok(entries) => (archived.session.branch.clone(), entries),
                Err(e) => return inner.refuse(COMMAND, format!("Could not read the transcript: {e}")),
            },
            None => return inner.refuse(COMMAND, crate::no_session(&id)),
        },
    };
    let dir = &inner.runtime.export_dir;
    let slug: String = branch
        .unwrap_or_else(|| "session".into())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    let path = dir.join(format!("{slug}-{id}.jsonl"));
    let written = std::fs::create_dir_all(dir).and_then(|_| {
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path)?);
        for entry in &entries {
            serde_json::to_writer(&mut file, entry as &Entry)?;
            file.write_all(b"\n")?;
        }
        file.flush()
    });
    match written {
        Ok(()) => inner.emit(Event::TranscriptExported { session_id: id.into(), path: path.to_string_lossy().into_owned() }),
        Err(e) => inner.refuse(COMMAND, format!("Could not write {}: {e}", path.display())),
    }
}

/// The `updateSettings` command.
pub(crate) fn update_settings(inner: &mut Inner, settings: Settings) {
    const COMMAND: &str = command_name::UPDATE_SETTINGS;
    if settings.archive_keep_per_repository == 0 || settings.archive_keep_days == 0 {
        return inner.refuse(COMMAND, "Keep at least one archived session for at least one day".into());
    }
    if let Err(e) = session_store::save_settings(inner.store.db(), &settings) {
        return inner.refuse(COMMAND, format!("Could not save the settings: {e}"));
    }
    inner.sessions.settings = settings.clone();
    inner.emit(Event::SettingsChanged { settings });
    prune(inner);
}
