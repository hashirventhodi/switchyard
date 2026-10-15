//! Hunk feedback: Accept / Reject / Comment on single hunks, and sending the rejected hunks
//! and comments to the worktree's agent as one structured message. Switchyard never
//! reverts or stages a hunk itself: a rejection only keeps its file out of commits until
//! the agent revises the hunk (its id changes, so the review lapses) or the user accepts
//! the whole file.

use std::fmt::Write as _;

use switchyard_schema::{ChangedFile, Decision, Event, Hunk, LineKind, SessionStatus};

use super::{changes, conflict, known};
use crate::{sessions, Inner};

/// One hunk's review. `sent`: the rejection and comment as they stand reached the agent.
#[derive(Clone, Default)]
pub(crate) struct HunkReview {
    decision: Option<Decision>,
    comment: Option<String>,
    sent: bool,
}

impl HunkReview {
    pub(super) fn apply(&self, hunk: &mut Hunk) {
        hunk.decision = self.decision;
        hunk.comment = self.comment.clone();
        hunk.sent = self.sent;
    }

    /// Something the agent has not been told yet.
    fn pending(&self) -> bool {
        !self.sent && (self.decision == Some(Decision::Reject) || self.comment.is_some())
    }
}

/// Look up a hunk of a changed file, refusing `command` when it is not (or no longer) there.
fn find(inner: &mut Inner, command: &str, worktree_path: &str, path: &str, hunk_id: &str) -> Option<ChangedFile> {
    known(inner, command, worktree_path)?;
    let list = match changes(inner, worktree_path) {
        Ok(list) => list,
        Err(reason) => {
            inner.refuse(command, reason);
            return None;
        }
    };
    let Some((file, _)) = list.into_iter().find(|(f, _)| f.path == path) else {
        inner.refuse(command, format!("{path} has no uncommitted changes"));
        return None;
    };
    if !file.hunks.iter().any(|h| h.id == hunk_id) {
        inner.refuse(command, format!("That hunk is no longer in {path}; the agent may have changed it"));
        return None;
    }
    Some(file)
}

/// Change one hunk's review and report it.
fn update(inner: &mut Inner, worktree_path: &str, path: &str, hunk_id: &str, change: impl FnOnce(&mut HunkReview)) {
    let reviews = inner.review.hunks.entry(worktree_path.to_string()).or_default();
    let review = reviews.entry(hunk_id.to_string()).or_default();
    change(review);
    let review = review.clone();
    if review.decision.is_none() && review.comment.is_none() {
        reviews.remove(hunk_id);
    }
    inner.emit(Event::HunkReviewed {
        worktree_path: worktree_path.into(),
        path: path.into(),
        hunk_id: hunk_id.into(),
        decision: review.decision,
        comment: review.comment,
        sent: review.sent,
    });
}

pub(crate) fn decide_hunk(inner: &mut Inner, worktree_path: &str, path: &str, hunk_id: &str, decision: Option<Decision>) {
    const COMMAND: &str = "decideHunk";
    let Some(file) = find(inner, COMMAND, worktree_path, path, hunk_id) else { return };
    if decision.is_some() {
        if let Some(reason) = conflict(inner, worktree_path, &file) {
            return inner.refuse(COMMAND, reason);
        }
    }
    // A rejection always lands: it takes back an Accept on the whole file.
    if decision == Some(Decision::Reject) && file.decision == Some(Decision::Accept) {
        if let Some(decided) = inner.review.by_worktree.get_mut(worktree_path) {
            decided.remove(path);
        }
        inner.emit(Event::FileDecided { worktree_path: worktree_path.into(), path: path.into(), decision: None });
    }
    update(inner, worktree_path, path, hunk_id, |r| {
        if r.decision != decision {
            r.decision = decision;
            r.sent = false;
        }
    });
}

pub(crate) fn comment_hunk(inner: &mut Inner, worktree_path: &str, path: &str, hunk_id: &str, comment: Option<&str>) {
    const COMMAND: &str = "commentHunk";
    if find(inner, COMMAND, worktree_path, path, hunk_id).is_none() {
        return;
    }
    let comment = comment.map(str::trim).filter(|c| !c.is_empty()).map(String::from);
    update(inner, worktree_path, path, hunk_id, |r| {
        if r.comment != comment {
            r.comment = comment;
            r.sent = false;
        }
    });
}

/// Accepting a whole file clears the rejections on its hunks (comments stay).
pub(super) fn clear_rejections(inner: &mut Inner, worktree_path: &str, file: &ChangedFile) {
    for hunk in file.hunks.iter().filter(|h| h.decision == Some(Decision::Reject)) {
        update(inner, worktree_path, &file.path, &hunk.id, |r| r.decision = None);
    }
}

/// The most diff lines quoted per hunk; the agent can read the rest from the file.
const QUOTED_LINES: usize = 12;

pub(crate) fn send_feedback(inner: &mut Inner, worktree_path: &str) {
    const COMMAND: &str = "sendFeedback";
    if known(inner, COMMAND, worktree_path).is_none() {
        return;
    }
    let list = match changes(inner, worktree_path) {
        Ok(list) => list,
        Err(reason) => return inner.refuse(COMMAND, reason),
    };
    let reviews = inner.review.hunks.get(worktree_path).cloned().unwrap_or_default();
    let items: Vec<(&ChangedFile, &Hunk)> = list
        .iter()
        .flat_map(|(f, _)| f.hunks.iter().map(move |h| (f, h)))
        .filter(|(_, h)| reviews.get(&h.id).is_some_and(HunkReview::pending))
        .collect();
    if items.is_empty() {
        return inner.refuse(COMMAND, "No hunk feedback to send: reject or comment on a hunk first".into());
    }
    let Some(session_id) = sessions::occupant_id(inner, worktree_path) else {
        return inner.refuse(COMMAND, "No session on this worktree to send feedback to".into());
    };
    // Checked here so a refused message never marks the feedback as sent.
    let reason = match sessions::occupant(inner, worktree_path) {
        Some(SessionStatus::Running) => Some("The agent is still working; wait for it or stop it first"),
        Some(SessionStatus::Idle) => Some("The session is paused; resume it first"),
        _ if sessions::awaits_request(inner, worktree_path) => Some("Answer the agent's request first: Run once or Skip"),
        _ => None,
    };
    if let Some(reason) = reason {
        return inner.refuse(COMMAND, reason.into());
    }
    sessions::send_prompt(inner, &session_id, &message(&items));
    let count = items.len() as u32;
    let sent: Vec<(String, String)> = items.iter().map(|(f, h)| (f.path.clone(), h.id.clone())).collect();
    for (path, hunk_id) in sent {
        update(inner, worktree_path, &path, &hunk_id, |r| r.sent = true);
    }
    inner.emit(Event::FeedbackSent { worktree_path: worktree_path.into(), session_id, hunks: count });
}

/// One message naming each file, hunk, verdict and comment, with the hunk's changed lines.
fn message(items: &[(&ChangedFile, &Hunk)]) -> String {
    let mut text = String::from(
        "Review feedback on your uncommitted changes. I have not changed any files: revise these hunks \
         yourself, then stop so I can review again.\n",
    );
    let mut last_path = "";
    for (file, hunk) in items {
        if file.path != last_path {
            let _ = write!(text, "\n## {}\n", file.path);
            last_path = &file.path;
        }
        let verdict = if hunk.decision == Some(Decision::Reject) { "Rejected" } else { "Comment on" };
        let _ = write!(text, "\n{verdict} hunk `{}`\n", hunk.header);
        if let Some(comment) = &hunk.comment {
            for line in comment.lines() {
                let _ = writeln!(text, "> {line}");
            }
        }
        let changed: Vec<String> = hunk
            .lines
            .iter()
            .filter(|l| l.kind != LineKind::Context)
            .map(|l| format!("{}{}", if l.kind == LineKind::Added { '+' } else { '-' }, l.text))
            .collect();
        text.push_str("```diff\n");
        for line in changed.iter().take(QUOTED_LINES) {
            let _ = writeln!(text, "{line}");
        }
        if changed.len() > QUOTED_LINES {
            let _ = writeln!(text, "… {} more changed lines", changed.len() - QUOTED_LINES);
        }
        text.push_str("```\n");
    }
    text
}
