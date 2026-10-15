//! Keeping a worktree's branch on top of its base: rebase it, ask its agent to, and read
//! the commits it has above base. Ahead/behind and conflicting files are worktree facts
//! (see `worktrees`). Git is the source of truth; nothing here is stored.

use crate::Inner;

pub(crate) fn rebase(inner: &mut Inner, worktree_path: &str) {
    let _ = (inner, worktree_path);
}

pub(crate) fn ask_agent(inner: &mut Inner, session_id: &str) {
    let _ = (inner, session_id);
}

pub(crate) fn history(inner: &Inner, worktree_path: &str) {
    let _ = (inner, worktree_path);
}
