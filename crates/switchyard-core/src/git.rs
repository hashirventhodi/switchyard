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

/// True when `dir` is inside a git repository (any of its worktrees).
pub fn is_inside_repository(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--git-dir"]).is_ok()
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

/// The commit-ish that stands for `base` in this repository: the local branch when it
/// exists, otherwise `origin/<base>`, otherwise `base` as given (HEAD, say).
pub fn base_ref(dir: &Path, base: &str) -> String {
    let local = format!("refs/heads/{base}");
    let remote = format!("refs/remotes/origin/{base}");
    if git(dir, &["show-ref", "--verify", "--quiet", &local]).is_ok() {
        base.to_string()
    } else if git(dir, &["show-ref", "--verify", "--quiet", &remote]).is_ok() {
        format!("origin/{base}")
    } else {
        base.to_string()
    }
}

/// True when `name` is acceptable as a new branch name.
pub fn is_valid_branch_name(dir: &Path, name: &str) -> bool {
    git(dir, &["check-ref-format", "--branch", name]).is_ok()
}

pub fn branch_exists(dir: &Path, name: &str) -> bool {
    git(dir, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{name}")]).is_ok()
}

/// `git worktree add -b <branch> <path> <start>`, run in the repository at `dir`.
pub fn add_worktree(dir: &Path, branch: &str, path: &Path, start: &str) -> Result<(), GitError> {
    let path = path.to_str().ok_or_else(|| GitError(format!("{} is not valid UTF-8", path.display())))?;
    git(dir, &["worktree", "add", "-q", "-b", branch, path, start]).map(|_| ())
}

/// `git worktree remove <path>`: git itself refuses a worktree with local changes.
pub fn remove_worktree(dir: &Path, path: &Path) -> Result<(), GitError> {
    let path = path.to_str().ok_or_else(|| GitError(format!("{} is not valid UTF-8", path.display())))?;
    git(dir, &["worktree", "remove", path]).map(|_| ())
}

/// Files with uncommitted changes in the worktree at `dir`, untracked files included.
pub fn dirty_count(dir: &Path) -> Result<u32, GitError> {
    let out = git(dir, &["status", "--porcelain", "--untracked-files=all"])?;
    Ok(out.lines().filter(|l| !l.trim().is_empty()).count() as u32)
}

/// Files a merge of `base` into HEAD would conflict on, by a dry-run merge
/// (`git merge-tree --write-tree`): it writes objects only, never the working tree, the
/// index or any ref.
pub fn conflicting_files(dir: &Path, base: &str) -> Result<Vec<String>, GitError> {
    let out = Command::new("git")
        .args(["merge-tree", "--write-tree", "--name-only", "--no-messages", "HEAD", base])
        .current_dir(dir)
        .output()
        .map_err(|e| GitError(format!("could not run git: {e}")))?;
    // Exit 0: merges cleanly. Exit 1: conflicts, listed after the tree id. Else: an error.
    match out.status.code() {
        Some(0) => Ok(Vec::new()),
        Some(1) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut files: Vec<String> = stdout.lines().skip(1).filter(|l| !l.is_empty()).map(str::to_owned).collect();
            files.dedup();
            Ok(files)
        }
        _ => Err(GitError(String::from_utf8_lossy(&out.stderr).trim().to_string())),
    }
}

/// (ahead, behind): commits HEAD has that `base` lacks, and the other way round.
pub fn ahead_behind(dir: &Path, base: &str) -> Result<(u32, u32), GitError> {
    let out = git(dir, &["rev-list", "--left-right", "--count", &format!("{base}...HEAD")])?;
    let mut counts = out.split_whitespace().map(|n| n.parse::<u32>().unwrap_or(0));
    let behind = counts.next().unwrap_or(0);
    let ahead = counts.next().unwrap_or(0);
    Ok((ahead, behind))
}
