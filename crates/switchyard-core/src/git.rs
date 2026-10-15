//! Reads repository facts from git by running the `git` CLI. Git is the source of truth:
//! nothing here is cached.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub primary: bool,
}

#[derive(Debug)]
pub struct GitError(pub String);

fn git(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| GitError(format!("could not run git: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(GitError(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

/// Every worktree `git worktree list` reports for the repository containing `dir`,
/// primary first. Bare entries are skipped.
pub fn worktrees(dir: &Path) -> Result<Vec<GitWorktree>, GitError> {
    let out = git(dir, &["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    for block in out.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut path = None;
        let mut branch = None;
        let mut bare = false;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            } else if line == "bare" {
                bare = true;
            }
        }
        if let (Some(path), false) = (path, bare) {
            let primary = list.is_empty();
            list.push(GitWorktree { path, branch, primary });
        }
    }
    Ok(list)
}

/// The branch work is based on: the remote's default branch when `origin/HEAD` is set,
/// otherwise `main` or `master` if present, otherwise the primary worktree's branch.
pub fn base_branch(dir: &Path, primary: &GitWorktree) -> String {
    if let Ok(remote_head) = git(dir, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        let remote_head = remote_head.trim();
        if let Some(branch) = remote_head.strip_prefix("origin/") {
            return branch.to_string();
        }
    }
    for candidate in ["main", "master"] {
        let refname = format!("refs/heads/{candidate}");
        if git(dir, &["show-ref", "--verify", "--quiet", &refname]).is_ok() {
            return candidate.to_string();
        }
    }
    primary.branch.clone().unwrap_or_else(|| "HEAD".to_string())
}
