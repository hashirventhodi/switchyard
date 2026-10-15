//! Review: a worktree's uncommitted changes, file decisions, and committing accepted files
//! whole. Switchyard never reverts anything and never stages part of a file. Decisions
//! live in memory keyed by worktree + path, pinned to the diff they were taken on, and are
//! discarded on commit.

mod diff;

use std::collections::HashMap;
use std::path::Path;

use switchyard_schema::{ChangedFile, Decision, Event, FileStatus};

use crate::{git, worktrees, Inner};

#[derive(Default)]
pub(crate) struct Decisions {
    /// worktree path → file path → (decision, fingerprint of the diff it was taken on).
    by_worktree: HashMap<String, HashMap<String, (Decision, u64)>>,
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
                    hunks: Vec::new(),
                },
            });
        }
    }
    let decided = inner.review.by_worktree.entry(worktree_path.to_string()).or_default();
    // A decision on a diff that has since changed (or a file no longer changed) is stale.
    decided.retain(|path, (_, fp)| parsed.iter().any(|p| &p.file.path == path && p.fingerprint == *fp));
    Ok(parsed
        .into_iter()
        .map(|mut p| {
            p.file.decision = decided.get(&p.file.path).map(|(d, _)| *d);
            (p.file, p.fingerprint)
        })
        .collect())
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
    const COMMAND: &str = "readChanges";
    if known(inner, COMMAND, worktree_path).is_none() {
        return;
    }
    emit_changes(inner, COMMAND, worktree_path);
}

fn emit_changes(inner: &mut Inner, command: &str, worktree_path: &str) {
    match changes(inner, worktree_path) {
        Ok(list) => {
            let files = list.into_iter().map(|(f, _)| f).collect();
            inner.emit(Event::ChangesRead { worktree_path: worktree_path.into(), files });
            worktrees::refresh_one(inner, worktree_path);
        }
        Err(reason) => inner.refuse(command, reason),
    }
}

pub(crate) fn decide(inner: &mut Inner, worktree_path: &str, path: &str, decision: Option<Decision>) {
    const COMMAND: &str = "decideFile";
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
    if file.status == FileStatus::Conflicted && decision.is_some() {
        return inner.refuse(COMMAND, format!("{path} has a conflict; resolve it before deciding"));
    }
    let decided = inner.review.by_worktree.entry(worktree_path.to_string()).or_default();
    match decision {
        Some(d) => decided.insert(path.to_string(), (d, fingerprint)),
        None => decided.remove(path),
    };
    inner.emit(Event::FileDecided { worktree_path: worktree_path.into(), path: path.into(), decision });
}

pub(crate) fn commit(inner: &mut Inner, worktree_path: &str, message: &str) {
    const COMMAND: &str = "commitAccepted";
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
    let accepted: Vec<&ChangedFile> =
        list.iter().map(|(f, _)| f).filter(|f| f.decision == Some(Decision::Accept)).collect();
    if accepted.is_empty() {
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
    inner.review.by_worktree.remove(worktree_path);
    inner.emit(Event::Committed {
        worktree_path: worktree_path.into(),
        branch: worktree.branch,
        commit,
        files: accepted.iter().map(|f| f.path.clone()).collect(),
    });
    emit_changes(inner, COMMAND, worktree_path);
}
