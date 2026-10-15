//! Keeping a worktree's branch on top of its base: rebase it, ask its agent to, and read
//! the commits it has above base. Ahead/behind and conflicting files are worktree facts
//! (see `worktrees`). Git is the source of truth; nothing here is stored.

use std::path::Path;

use switchyard_schema::{Event, SessionStatus};

use crate::{git, sessions, worktrees, Inner};

/// The base branch name of the repository whose primary worktree is at `repo`.
fn base_of(repo: &Path) -> Option<String> {
    let list = git::worktrees(repo).ok()?;
    Some(git::base_branch(repo, list.first()?))
}

pub(crate) fn rebase(inner: &mut Inner, worktree_path: &str) {
    const COMMAND: &str = "rebaseWorktree";
    let Some((repo, worktree)) = inner.find_worktree(worktree_path) else {
        return inner.refuse(COMMAND, format!("{worktree_path} is not a worktree of an added repository"));
    };
    let Some(base) = base_of(Path::new(&repo.path)) else {
        return inner.refuse(COMMAND, format!("{} is no longer a readable git repository", repo.path));
    };
    let Some(name) = worktree.branch.clone() else {
        return inner.refuse(COMMAND, format!("{worktree_path} has no branch checked out (detached HEAD)"));
    };
    if worktree.primary {
        return inner.refuse(COMMAND, format!("{name} is the primary worktree; only branch worktrees are rebased on {base}"));
    }
    // Switchyard never rewrites the tree under a working agent.
    if let Some(status) = sessions::occupant(inner, worktree_path) {
        if status == SessionStatus::Running {
            return inner.refuse(
                COMMAND,
                format!("{name} has a Running session; stop it or ask the agent to rebase"),
            );
        }
    }
    match git::dirty_count(&worktree.path) {
        Ok(0) => {}
        Ok(1) => return inner.refuse(COMMAND, format!("{name} has 1 uncommitted file; commit or discard it before rebasing")),
        Ok(n) => return inner.refuse(COMMAND, format!("{name} has {n} uncommitted files; commit or discard them before rebasing")),
        Err(git::GitError(e)) => return inner.refuse(COMMAND, format!("git could not read {name}: {e}")),
    }
    let onto = git::base_ref(&worktree.path, &base);
    match git::rebase(&worktree.path, &onto) {
        Ok(git::Rebase::Done) => {
            inner.emit(Event::WorktreeRebased { worktree_path: worktree_path.into(), base: base.clone() });
        }
        Ok(git::Rebase::Aborted { conflicts }) => {
            inner.refuse(
                COMMAND,
                format!("Rebasing {name} on {base} stopped on conflicts in {}; aborted, the branch is unchanged", conflicts.join(", ")),
            );
        }
        Err(git::GitError(e)) => inner.refuse(COMMAND, format!("Could not rebase {name} on {base}: {e}")),
    }
    worktrees::emit_facts(inner, &worktree.path, &base);
}

pub(crate) fn ask_agent(inner: &mut Inner, session_id: &str) {
    let _ = (inner, session_id);
}

pub(crate) fn history(inner: &Inner, worktree_path: &str) {
    let _ = (inner, worktree_path);
}
