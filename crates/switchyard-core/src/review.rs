//! Review: a worktree's uncommitted changes, file decisions, and committing accepted files
//! whole. Switchyard never reverts anything and never stages part of a file. Decisions are
//! keyed by worktree + path, pinned to the content they were taken on, written through to
//! the store (so they survive a relaunch) and discarded on commit. Hunks carry their own
//! decisions and comments (Hunk feedback, see `feedback`), stored the same way. A Decision
//! whose content has changed since is a Stale decision: kept and reported, never counted.

mod conflicts;
mod diff;
mod feedback;

pub(crate) use conflicts::resolve as resolve_conflict;

pub(crate) use feedback::{comment_hunk, decide_hunk, send_feedback, HunkReview};

use std::collections::HashMap;
use std::path::Path;

use switchyard_schema::command_name;
use switchyard_schema::{ChangedFile, Decision, Event, FileStatus};

use crate::{git, session_store, worktrees, Inner};

#[derive(Default)]
pub(crate) struct Decisions {
    /// worktree path → file path → (decision, content hash of the file it was taken on: see
    /// `diff::Parsed::fingerprint`).
    by_worktree: HashMap<String, HashMap<String, (Decision, u64)>>,
    /// worktree path → hunk id → review. Hunk ids hash the hunk's own lines and index-side
    /// path, so a review goes stale exactly when the agent revises that hunk.
    hunks: HashMap<String, HashMap<String, feedback::HunkReview>>,
}

/// The file's own decision when it still counts (a Stale decision does not).
fn in_force(file: &ChangedFile) -> Option<Decision> {
    file.decision.filter(|_| !file.stale)
}

/// A file's commit eligibility, from its own decision and its hunks'.
fn committable(file: &ChangedFile) -> bool {
    match in_force(file) {
        Some(d) => d == Decision::Accept,
        None => !file.hunks.is_empty() && file.hunks.iter().all(|h| h.decision == Some(Decision::Accept)),
    }
}

/// A file accepted on content that has since changed: left out of commits until re-confirmed.
fn stale_accept(file: &ChangedFile) -> bool {
    file.stale && file.decision == Some(Decision::Accept) && !committable(file)
}

/// A file with a rejected hunk and no decision of its own: not committable until the agent
/// revises the hunk or the user accepts the whole file.
fn blocked(file: &ChangedFile) -> bool {
    in_force(file).is_none() && file.hunks.iter().any(|h| h.decision == Some(Decision::Reject))
}

/// Load the stored decisions (when the core opens).
pub(crate) fn load(inner: &mut Inner) {
    if let Ok(stored) = session_store::decisions(inner.store.db()) {
        inner.review.by_worktree = stored;
    }
    if let Ok(stored) = session_store::hunk_reviews(inner.store.db()) {
        inner.review.hunks = stored;
    }
}

/// Write the worktree's file decisions and hunk reviews through to the store.
pub(super) fn save(inner: &Inner, worktree_path: &str) {
    let decided = inner.review.by_worktree.get(worktree_path).cloned().unwrap_or_default();
    let _ = session_store::save_decisions(inner.store.db(), worktree_path, &decided);
    let reviews = inner.review.hunks.get(worktree_path).cloned().unwrap_or_default();
    let _ = session_store::save_hunk_reviews(inner.store.db(), worktree_path, &reviews);
}

/// Forget every decision on a worktree that is gone.
pub(crate) fn forget(inner: &mut Inner, worktree_path: &str) {
    inner.review.by_worktree.remove(worktree_path);
    inner.review.hunks.remove(worktree_path);
    save(inner, worktree_path);
}

/// Discard the worktree's Decisions on files that are no longer changed (committed or
/// reverted, in Switchyard or outside it), and hunk reviews that no longer apply: on such a
/// file, or a lone comment whose hunk is gone (`has_hunk` says which hunks the diff has).
/// Whether anything was discarded.
fn prune(inner: &mut Inner, worktree_path: &str, changed: &[&str], has_hunk: impl Fn(&str) -> bool) -> bool {
    let mut lapsed = false;
    if let Some(decided) = inner.review.by_worktree.get_mut(worktree_path) {
        let before = decided.len();
        decided.retain(|path, _| changed.contains(&path.as_str()));
        lapsed |= decided.len() != before;
    }
    if let Some(reviews) = inner.review.hunks.get_mut(worktree_path) {
        let before = reviews.len();
        reviews.retain(|id, r| has_hunk(id) || (r.decision.is_some() && changed.contains(&r.path.as_str())));
        lapsed |= reviews.len() != before;
    }
    lapsed
}

/// The worktree changed on disk: discard the Decisions on files git no longer lists as
/// changed (committed or reverted in a shell, say), without computing the diff. Nothing is
/// read from git for a worktree with nothing decided.
pub(crate) fn prune_settled(inner: &mut Inner, worktree_path: &str) {
    let decided = inner.review.by_worktree.get(worktree_path).is_some_and(|d| !d.is_empty())
        || inner.review.hunks.get(worktree_path).is_some_and(|r| !r.is_empty());
    if !decided {
        return;
    }
    let Ok(changed) = git::changed_paths(Path::new(worktree_path)) else { return };
    let changed: Vec<&str> = changed.iter().map(String::as_str).collect();
    // Without the diff, any hunk on a file still changed may still be there (as may one
    // stored before reviews named their file): left for the next read to judge.
    let keep: Vec<String> = inner
        .review
        .hunks
        .get(worktree_path)
        .into_iter()
        .flatten()
        .filter(|(_, r)| r.path.is_empty() || changed.contains(&r.path.as_str()))
        .map(|(id, _)| id.clone())
        .collect();
    if prune(inner, worktree_path, &changed, |id| keep.iter().any(|k| k == id)) {
        save(inner, worktree_path);
    }
}

/// Discard the Decisions and hunk reviews on `paths`, just committed.
fn discard(inner: &mut Inner, worktree_path: &str, paths: &[String]) {
    if let Some(decided) = inner.review.by_worktree.get_mut(worktree_path) {
        decided.retain(|path, _| !paths.contains(path));
    }
    if let Some(reviews) = inner.review.hunks.get_mut(worktree_path) {
        reviews.retain(|_, r| !paths.contains(&r.path));
    }
    save(inner, worktree_path);
}

/// Read the worktree's changes from git, with the decisions that still apply.
fn changes(inner: &mut Inner, worktree_path: &str) -> Result<Vec<(ChangedFile, u64)>, String> {
    let dir = Path::new(worktree_path);
    let patch = git::diff_against_index(dir).map_err(|git::GitError(e)| format!("git could not read the changes: {e}"))?;
    let mut parsed = diff::parse(&patch);
    for path in git::unmerged_paths(dir).map_err(|git::GitError(e)| format!("git could not read the changes: {e}"))? {
        if !parsed.iter().any(|p| p.file.path == path) {
            parsed.push(diff::Parsed {
                fingerprint: 0,
                file: ChangedFile {
                    path,
                    old_path: None,
                    status: FileStatus::Conflicted,
                    added: 0,
                    removed: 0,
                    binary: false,
                    decision: None,
                    stale: false,
                    hunks: Vec::new(),
                    stale_hunks: Vec::new(),
                },
            });
        }
    }
    // Decisions are kept when their content changes (a file reverted to it counts again);
    // those on a file no longer changed go, and a lone comment on a hunk that is gone lapses.
    let changed: Vec<&str> = parsed.iter().map(|p| p.file.path.as_str()).collect();
    let lapsed = prune(inner, worktree_path, &changed, |id| parsed.iter().any(|p| p.file.hunks.iter().any(|h| h.id == id)));
    let decided = inner.review.by_worktree.entry(worktree_path.to_string()).or_default();
    let reviews = inner.review.hunks.entry(worktree_path.to_string()).or_default();
    let mut list: Vec<(ChangedFile, u64)> = parsed
        .into_iter()
        .map(|mut p| {
            if let Some((d, fp)) = decided.get(&p.file.path) {
                p.file.decision = Some(*d);
                p.file.stale = *fp != p.fingerprint;
            }
            for hunk in &mut p.file.hunks {
                if let Some(r) = reviews.get(&hunk.id) {
                    r.apply(hunk);
                }
            }
            (p.file, p.fingerprint)
        })
        .collect();
    // A hunk the agent revised (or dropped) has a new id: its Decision is stale.
    let mut stale: Vec<(&String, &feedback::HunkReview)> = reviews
        .iter()
        .filter(|(id, _)| !list.iter().any(|(f, _)| f.hunks.iter().any(|h| &h.id == *id)))
        .collect();
    stale.sort_by(|a, b| a.1.header.cmp(&b.1.header).then(a.0.cmp(b.0)));
    for (id, review) in stale {
        if let Some((file, _)) = list.iter_mut().find(|(f, _)| f.path == review.path) {
            file.stale_hunks.extend(review.stale(id));
        }
    }
    if lapsed {
        save(inner, worktree_path);
    }
    Ok(list)
}

/// The base branch's name and the files a dry-run merge of it into the worktree conflicts
/// on (#17's conflicts fact; empty when not behind).
fn base_conflicts(inner: &Inner, worktree_path: &str) -> (String, Vec<String>) {
    let (base, base_ref, behind) = base_of(inner, worktree_path);
    let conflicts = if behind > 0 {
        git::conflicting_files(Path::new(worktree_path), &base_ref).unwrap_or_default()
    } else {
        Vec::new()
    };
    (base, conflicts)
}

/// The base branch's name, the commit-ish standing for it, and how far behind it the
/// worktree is (empty names and 0 when unknown).
fn base_of(inner: &Inner, worktree_path: &str) -> (String, String, u32) {
    let Some((repo, wt)) = inner.find_worktree(worktree_path) else { return (String::new(), String::new(), 0) };
    let dir = Path::new(&repo.path);
    let Some(primary) = git::worktrees(dir).ok().and_then(|l| l.into_iter().next()) else {
        return (String::new(), String::new(), 0);
    };
    let base = git::base_branch(dir, &primary);
    let base_ref = git::base_ref(&wt.path, &base);
    let behind = git::ahead_behind(&wt.path, &base_ref).map(|(_, b)| b).unwrap_or(0);
    (base, base_ref, behind)
}

/// Look up a worktree of an added repository, refusing `command` when there is none.
fn known(inner: &Inner, command: &str, worktree_path: &str) -> Option<git::GitWorktree> {
    match inner.find_worktree(worktree_path) {
        Some((_, wt)) => Some(wt),
        None => {
            inner.refuse(command, format!("{worktree_path} is not a worktree of an added repository"));
            None
        }
    }
}

pub(crate) fn read(inner: &mut Inner, worktree_path: &str) {
    const COMMAND: &str = command_name::READ_CHANGES;
    if known(inner, COMMAND, worktree_path).is_none() {
        return;
    }
    emit_changes(inner, COMMAND, worktree_path);
}

pub(crate) fn emit_changes(inner: &mut Inner, command: &str, worktree_path: &str) {
    match changes(inner, worktree_path) {
        Ok(list) => {
            let files = list.into_iter().map(|(f, _)| f).collect();
            inner.emit(Event::ChangesRead { worktree_path: worktree_path.into(), files });
            let (base, conflicts) = conflicts::read(inner, worktree_path);
            inner.emit(Event::ConflictsRead { worktree_path: worktree_path.into(), base, files: conflicts });
            worktrees::refresh_one(inner, worktree_path);
        }
        Err(reason) => inner.refuse(command, reason),
    }
}

/// Why `file` cannot be decided now: it is unmerged, or a merge of base would conflict on it.
fn conflict(inner: &Inner, worktree_path: &str, file: &ChangedFile) -> Option<String> {
    let path = &file.path;
    if file.status == FileStatus::Conflicted {
        return Some(format!("{path} has a conflict; resolve it before deciding"));
    }
    let (base, conflicts) = base_conflicts(inner, worktree_path);
    conflicts.iter().any(|c| c == path).then(|| format!("{path} conflicts with {base}; rebase before deciding"))
}

pub(crate) fn decide(inner: &mut Inner, worktree_path: &str, path: &str, decision: Option<Decision>) {
    const COMMAND: &str = command_name::DECIDE_FILE;
    if known(inner, COMMAND, worktree_path).is_none() {
        return;
    }
    let list = match changes(inner, worktree_path) {
        Ok(list) => list,
        Err(reason) => return inner.refuse(COMMAND, reason),
    };
    let Some((file, fingerprint)) = list.into_iter().find(|(f, _)| f.path == path) else {
        return inner.refuse(COMMAND, format!("{path} has no uncommitted changes"));
    };
    if decision.is_some() {
        if let Some(reason) = conflict(inner, worktree_path, &file) {
            return inner.refuse(COMMAND, reason);
        }
    }
    let decided = inner.review.by_worktree.entry(worktree_path.to_string()).or_default();
    match decision {
        Some(d) => decided.insert(path.to_string(), (d, fingerprint)),
        None => decided.remove(path),
    };
    save(inner, worktree_path);
    inner.emit(Event::FileDecided { worktree_path: worktree_path.into(), path: path.into(), decision });
    if decision == Some(Decision::Accept) {
        // Accepting the whole file overrides its hunk rejections.
        feedback::clear_rejections(inner, worktree_path, &file);
    }
}

pub(crate) fn commit(inner: &mut Inner, worktree_path: &str, message: &str) {
    const COMMAND: &str = command_name::COMMIT_ACCEPTED;
    let Some(worktree) = known(inner, COMMAND, worktree_path) else { return };
    let message = message.trim();
    if message.is_empty() {
        return inner.refuse(COMMAND, "Write a commit message first".into());
    }
    let list = match changes(inner, worktree_path) {
        Ok(list) => list,
        Err(reason) => return inner.refuse(COMMAND, reason),
    };
    if let Some((conflicted, _)) = list.iter().find(|(f, _)| f.status == FileStatus::Conflicted) {
        return inner.refuse(COMMAND, format!("Resolve the conflict in {} before committing", conflicted.path));
    }
    if let Some(path) = base_conflicts(inner, worktree_path).1.first() {
        return inner.refuse(COMMAND, format!("Resolve the conflict in {path} before committing"));
    }
    let accepted: Vec<&ChangedFile> = list.iter().map(|(f, _)| f).filter(|f| committable(f)).collect();
    let skipped_stale: Vec<String> = list.iter().map(|(f, _)| f).filter(|f| stale_accept(f)).map(|f| f.path.clone()).collect();
    if accepted.is_empty() {
        if !skipped_stale.is_empty() {
            // Every accepted file changed since you decided: nothing to commit, and why.
            return inner.emit(Event::Committed {
                worktree_path: worktree_path.into(),
                branch: worktree.branch,
                commit: String::new(),
                files: Vec::new(),
                skipped_stale,
            });
        }
        if let Some((f, _)) = list.iter().find(|(f, _)| blocked(f)) {
            return inner.refuse(
                COMMAND,
                format!("{} has a rejected hunk: send feedback to the agent or accept the whole file", f.path),
            );
        }
        return inner.refuse(COMMAND, "Accept at least one file to commit".into());
    }
    let dir = Path::new(worktree_path);
    // Staged changes are outside the diff the user reviewed; committing the file would
    // carry them along unseen.
    let staged = match git::staged_paths(dir) {
        Ok(s) => s,
        Err(git::GitError(e)) => return inner.refuse(COMMAND, format!("git could not read the index: {e}")),
    };
    let mut paths = Vec::new();
    for f in &accepted {
        paths.push(f.path.clone());
        paths.extend(f.old_path.clone());
    }
    if let Some(p) = paths.iter().find(|p| staged.contains(p)) {
        return inner.refuse(COMMAND, format!("{p} has staged changes that were not reviewed; unstage them first"));
    }
    let commit = match git::commit_paths(dir, &paths, message) {
        Ok(c) => c,
        Err(git::GitError(e)) => return inner.refuse(COMMAND, format!("Could not commit: {e}")),
    };
    // Decisions on what was not committed (a skipped Stale decision, say) stay.
    discard(inner, worktree_path, &paths);
    inner.emit(Event::Committed {
        worktree_path: worktree_path.into(),
        branch: worktree.branch,
        commit,
        files: accepted.iter().map(|f| f.path.clone()).collect(),
        skipped_stale,
    });
    emit_changes(inner, COMMAND, worktree_path);
}
