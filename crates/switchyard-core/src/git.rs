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

/// One commit as `log` reads it.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Committer time, seconds since the Unix epoch.
    pub time: u64,
}

/// Commits on HEAD that `base` lacks, newest first, capped at `limit`.
pub fn log_above(dir: &Path, base: &str, limit: usize) -> Result<Vec<LogEntry>, GitError> {
    let max = format!("--max-count={limit}");
    let out = git(dir, &["log", &max, "--format=%H%x1f%h%x1f%an%x1f%ct%x1f%s", &format!("{base}..HEAD")])?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut f = line.splitn(5, '\u{1f}');
            Some(LogEntry {
                hash: f.next()?.into(),
                short: f.next()?.into(),
                author: f.next()?.into(),
                time: f.next()?.parse().unwrap_or(0),
                subject: f.next().unwrap_or_default().into(),
            })
        })
        .collect())
}

/// How a rebase ended.
#[derive(Debug)]
pub enum Rebase {
    Done,
    /// It stopped on conflicts in these files and was aborted: the branch, working tree
    /// and index are as they were.
    Aborted { conflicts: Vec<String> },
}

/// `git rebase <onto>` in the worktree at `dir`, never interactive and never stashing.
/// A rebase that stops is aborted at once.
pub fn rebase(dir: &Path, onto: &str) -> Result<Rebase, GitError> {
    let out = Command::new("git")
        .args(["-c", "rebase.autoStash=false", "rebase", "--no-autosquash", onto])
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .current_dir(dir)
        .output()
        .map_err(|e| GitError(format!("could not run git: {e}")))?;
    if out.status.success() {
        return Ok(Rebase::Done);
    }
    let conflicts = git(dir, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    let conflicts: Vec<String> = conflicts.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect();
    // A rebase that never started (say, git refused it) has nothing to abort.
    let started = git(dir, &["rev-parse", "--git-path", "rebase-merge"])
        .into_iter()
        .chain(git(dir, &["rev-parse", "--git-path", "rebase-apply"]))
        .any(|p| {
            let p = PathBuf::from(p.trim());
            (if p.is_absolute() { p } else { dir.join(p) }).exists()
        });
    if started {
        git(dir, &["rebase", "--abort"])?;
        Ok(Rebase::Aborted { conflicts })
    } else {
        Err(GitError(String::from_utf8_lossy(&out.stderr).trim().to_string()))
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

/// Run git in `dir` with extra environment and no optional index writes.
fn git_with(dir: &Path, args: &[&str], env: &[(&str, &Path)]) -> Result<String, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir).env("GIT_OPTIONAL_LOCKS", "0");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().map_err(|e| GitError(format!("could not run git: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(GitError(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

fn nul_list(out: &str) -> Vec<String> {
    out.split('\0').filter(|p| !p.is_empty()).map(str::to_owned).collect()
}

/// The worktree's patch against its index, untracked files shown as added (and paired
/// with deleted files as renames). The real index is never written: untracked files are
/// marked intent-to-add in a throwaway copy of it.
pub fn diff_against_index(dir: &Path) -> Result<String, GitError> {
    const DIFF: &[&str] = &[
        "-c", "core.quotePath=false", "-c", "diff.noprefix=false", "-c", "diff.mnemonicPrefix=false",
        "diff", "--no-color", "--no-ext-diff", "--no-textconv", "--full-index", "-M",
        "--src-prefix=a/", "--dst-prefix=b/",
    ];
    let untracked = nul_list(&git(dir, &["ls-files", "-z", "--others", "--exclude-standard"])?);
    if untracked.is_empty() {
        return git_with(dir, DIFF, &[]);
    }
    let scratch = Scratch::new()?;
    let index = scratch.0.join("index");
    let real = PathBuf::from(git(dir, &["rev-parse", "--path-format=absolute", "--git-path", "index"])?.trim());
    if real.exists() {
        std::fs::copy(&real, &index).map_err(|e| GitError(format!("cannot copy the index: {e}")))?;
    }
    let list = scratch.0.join("paths");
    std::fs::write(&list, untracked.join("\0")).map_err(|e| GitError(e.to_string()))?;
    let list_arg = format!("--pathspec-from-file={}", list.display());
    git_with(dir, &["add", "-N", "--pathspec-file-nul", &list_arg], &[("GIT_INDEX_FILE", &index)])?;
    git_with(dir, DIFF, &[("GIT_INDEX_FILE", &index)])
}

/// Paths git holds unmerged (a conflict stopped a merge or rebase here).
pub fn unmerged_paths(dir: &Path) -> Result<Vec<String>, GitError> {
    let out = git(dir, &["-c", "core.quotePath=false", "diff", "--name-only", "-z", "--diff-filter=U"])?;
    let mut list = nul_list(&out);
    list.dedup();
    Ok(list)
}

/// Paths whose index entry differs from HEAD (staged changes).
pub fn staged_paths(dir: &Path) -> Result<Vec<String>, GitError> {
    // A repository without commits has nothing to compare against: everything staged.
    if git(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err() {
        return Ok(nul_list(&git(dir, &["ls-files", "-z"])?));
    }
    Ok(nul_list(&git(dir, &["-c", "core.quotePath=false", "diff", "--cached", "--name-only", "-z", "--no-renames"])?))
}

/// Stage `paths` whole (additions, edits and deletions) and commit exactly them. Other
/// staged paths stay staged and are not committed. Returns the abbreviated commit id. On
/// failure the paths are unstaged again; the working tree is never touched.
pub fn commit_paths(dir: &Path, paths: &[String], message: &str) -> Result<String, GitError> {
    let scratch = Scratch::new()?;
    let list = scratch.0.join("paths");
    std::fs::write(&list, paths.join("\0")).map_err(|e| GitError(e.to_string()))?;
    let list_arg = format!("--pathspec-from-file={}", list.display());
    let msg = scratch.0.join("message");
    std::fs::write(&msg, message).map_err(|e| GitError(e.to_string()))?;
    let msg_arg = format!("--file={}", msg.display());
    git(dir, &["add", "-A", "--pathspec-file-nul", &list_arg])?;
    let committed =
        git(dir, &["commit", "-q", "--only", "--cleanup=strip", &msg_arg, "--pathspec-file-nul", &list_arg]);
    if let Err(e) = committed {
        let _ = git(dir, &["reset", "-q", "--pathspec-file-nul", &list_arg]);
        return Err(e);
    }
    Ok(git(dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string())
}

/// A private temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, GitError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let name = format!("switchyard-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed));
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).map_err(|e| GitError(format!("cannot create {}: {e}", dir.display())))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
