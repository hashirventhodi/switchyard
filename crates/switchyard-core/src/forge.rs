//! The Forge (ADR-0003): work leaves a worktree through a pull request. Open pull request
//! pushes the branch and opens one; every worktree branch of a Repository with a remote is
//! then polled on the Forge until its pull request closes or is Merged, whether or not it
//! was opened from Switchyard. Merged cleans the worktree up (removed, shells closed,
//! Session archived). Prune merged does the same for every free, clean, Merged worktree,
//! and Local merge stands in for all of it in a Repository with no remote.
//!
//! Polling is ticked from outside ([`crate::Core::poll_forge`]; the app ticks every
//! [`crate::Runtime::forge_poll_every`]). A worktree is polled on the first tick after it
//! appears or opens a pull request; each poll that sees no change doubles the ticks until
//! the next one, up to [`MAX_TICKS`], and any change polls again on the next tick. Nothing
//! is stored: after a relaunch every branch is looked up again.

mod gh;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use switchyard_schema::{
    ChecksState, Event, ForgeSetup, ForgeSetupState, Link, PullRequest, PullRequestState, ReviewState,
    SessionStatus,
};

pub use gh::GhForge;

use crate::{git, sessions, worktrees, Inner};

/// The Forge as the core uses it. The app uses [`GhForge`]; tests run it against a fake `gh`.
pub trait Forge: Send + Sync {
    /// Is the CLI installed and signed in?
    fn detect(&self) -> ForgeSetup;
    /// Open a pull request from `branch` (already pushed) into `base`, from the worktree at
    /// `dir`. Returns its address.
    fn create_pull_request(&self, dir: &Path, base: &str, branch: &str) -> Result<String, String>;
    /// The pull request for `branch` (an open one if any, else the newest), if there is one.
    fn pull_request(&self, dir: &Path, branch: &str) -> Result<Option<ForgePullRequest>, String>;
}

/// A pull request as polled, plus the commit its head was at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgePullRequest {
    pub pull_request: PullRequest,
    pub head_oid: String,
}

/// No Forge configured: pull requests cannot be opened.
pub struct NoForge;

impl Forge for NoForge {
    fn detect(&self) -> ForgeSetup {
        ForgeSetup {
            name: "GitHub".into(),
            state: ForgeSetupState::Missing {
                install_hint: "brew install gh".into(),
                link: Link { label: "Install GitHub CLI".into(), url: "https://cli.github.com".into() },
            },
        }
    }
    fn create_pull_request(&self, _: &Path, _: &str, _: &str) -> Result<String, String> {
        Err("no Forge is configured".into())
    }
    fn pull_request(&self, _: &Path, _: &str) -> Result<Option<ForgePullRequest>, String> {
        Err("no Forge is configured".into())
    }
}

/// The most ticks between two polls of one worktree.
pub const MAX_TICKS: u32 = 8;

#[derive(Default)]
pub(crate) struct Tracker {
    setup: Option<ForgeSetup>,
    /// By worktree path.
    tracks: HashMap<String, Track>,
}

struct Track {
    pull_request: Option<ForgePullRequest>,
    /// Ticks between polls now, and ticks left until the next one.
    every: u32,
    wait: u32,
    /// The pull request closed or merged: no more polling until one is opened again.
    stopped: bool,
}

impl Default for Track {
    fn default() -> Self {
        Track { pull_request: None, every: 1, wait: 0, stopped: false }
    }
}

impl Track {
    fn polled(&mut self, changed: bool) {
        self.every = if changed { 1 } else { (self.every * 2).min(MAX_TICKS) };
        self.wait = self.every - 1;
    }
}

/// On attach: the last detection, detecting first if nothing has been yet.
pub(crate) fn replay_setup(inner: &mut Inner) {
    if inner.forge.setup.is_none() {
        detect(inner);
    }
    if let Some(forge) = inner.forge.setup.clone() {
        inner.emit(Event::ForgeChecked { forge });
    }
}

/// On attach, after the worktrees: every pull request known.
pub(crate) fn replay_pull_requests(inner: &Inner) {
    let mut known: Vec<_> = inner
        .forge
        .tracks
        .iter()
        .filter_map(|(path, t)| t.pull_request.as_ref().map(|p| (path.clone(), p.pull_request.clone())))
        .collect();
    known.sort_by(|a, b| a.0.cmp(&b.0));
    for (worktree_path, pull_request) in known {
        inner.emit(Event::PullRequestChanged { worktree_path, pull_request: Some(pull_request) });
    }
}

/// [`Command::CheckForge`](switchyard_schema::Command::CheckForge).
pub(crate) fn check(inner: &mut Inner) {
    detect(inner);
    let forge = inner.forge.setup.clone().expect("just detected");
    inner.emit(Event::ForgeChecked { forge });
}

fn detect(inner: &mut Inner) {
    inner.forge.setup = Some(inner.runtime.forge.detect());
}

/// Can the Forge be used? Detects again every time (the user may have installed, signed in
/// or signed out since), announcing a change. `Err` is the reason for the user.
fn admit(inner: &mut Inner) -> Result<(), String> {
    let before = inner.forge.setup.clone();
    detect(inner);
    if inner.forge.setup != before {
        inner.emit(Event::ForgeChecked { forge: inner.forge.setup.clone().expect("just detected") });
    }
    match &inner.forge.setup.as_ref().expect("detected").state {
        ForgeSetupState::Ready { .. } => Ok(()),
        ForgeSetupState::Missing { install_hint, .. } => {
            Err(format!("GitHub CLI (gh) is not installed; install it ({install_hint}) to open pull requests"))
        }
        ForgeSetupState::SignedOut { login_hint } => {
            Err(format!("GitHub CLI (gh) is not signed in; run {login_hint}, then try again"))
        }
    }
}

/// The worktree at `path` is gone: stop polling it.
pub(crate) fn forget(inner: &mut Inner, path: &str) {
    inner.forge.tracks.remove(path);
}

/// The base branch of the repository whose primary worktree is at `repo`.
fn base_of(repo: &Path) -> Option<String> {
    let list = git::worktrees(repo).ok()?;
    Some(git::base_branch(repo, list.first()?))
}

fn plural(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

pub(crate) fn open_pull_request(inner: &mut Inner, worktree_path: &str) {
    const COMMAND: &str = "openPullRequest";
    let Some((repo, worktree)) = inner.find_worktree(worktree_path) else {
        return inner.refuse(COMMAND, format!("{worktree_path} is not a worktree of an added repository"));
    };
    let Some(branch) = worktree.branch.clone() else {
        return inner.refuse(COMMAND, format!("{worktree_path} has no branch checked out (detached HEAD)"));
    };
    if worktree.primary {
        return inner.refuse(COMMAND, format!("{branch} is the primary worktree; open pull requests from a branch worktree"));
    }
    let Some(remote) = git::remote(Path::new(&repo.path)) else {
        return inner.refuse(COMMAND, format!("{branch}'s repository has no remote; use Local merge instead"));
    };
    let Some(base) = base_of(Path::new(&repo.path)) else {
        return inner.refuse(COMMAND, format!("{} is no longer a readable git repository", repo.path));
    };
    match git::dirty_count(&worktree.path) {
        Ok(0) => {}
        Ok(n) => {
            let files = plural(n, "uncommitted file", "uncommitted files");
            return inner.refuse(COMMAND, format!("{branch} has {files}; commit them before opening a pull request"));
        }
        Err(git::GitError(e)) => return inner.refuse(COMMAND, format!("git could not read {branch}: {e}")),
    }
    let onto = git::base_ref(&worktree.path, &base);
    let (ahead, behind) = git::ahead_behind(&worktree.path, &onto).unwrap_or((0, 0));
    if behind > 0 {
        let commits = plural(behind, "commit", "commits");
        return inner.refuse(COMMAND, format!("{branch} is {commits} behind {base}; rebase before opening a pull request"));
    }
    if ahead == 0 {
        return inner.refuse(COMMAND, format!("{branch} has no commits above {base}; commit the work first"));
    }
    if let Err(reason) = admit(inner) {
        return inner.refuse(COMMAND, reason);
    }
    if let Some(open) = inner.forge.tracks.get(worktree_path).and_then(|t| t.pull_request.as_ref()) {
        if open.pull_request.state == PullRequestState::Open {
            let n = open.pull_request.number;
            return inner.refuse(COMMAND, format!("PR #{n} is already open for {branch}"));
        }
    }
    if let Err(git::GitError(e)) = git::push(&worktree.path, &remote, &branch) {
        return inner.refuse(COMMAND, format!("Could not push {branch} to {remote}: {e}"));
    }
    let forge = inner.runtime.forge.clone();
    let url = match forge.create_pull_request(&worktree.path, &base, &branch) {
        Ok(url) => url,
        Err(e) => return inner.refuse(COMMAND, format!("{branch} was pushed, but the pull request could not be opened: {e}")),
    };
    // Read it back for its checks and review; fall back to what the address says.
    let found = forge.pull_request(&worktree.path, &branch).ok().flatten().filter(|p| p.pull_request.url == url);
    let opened = found.unwrap_or_else(|| ForgePullRequest {
        pull_request: PullRequest {
            number: url.rsplit('/').next().and_then(|n| n.parse().ok()).unwrap_or(0),
            url: url.clone(),
            state: PullRequestState::Open,
            draft: false,
            checks: ChecksState::None,
            review: ReviewState::None,
        },
        head_oid: String::new(),
    });
    let pull_request = opened.pull_request.clone();
    inner.forge.tracks.insert(
        worktree_path.to_string(),
        Track { pull_request: Some(opened), ..Track::default() },
    );
    // Tasks (M4) move to In review here.
    inner.emit(Event::PullRequestOpened { worktree_path: worktree_path.into(), pull_request: pull_request.clone() });
    inner.emit(Event::PullRequestChanged { worktree_path: worktree_path.into(), pull_request: Some(pull_request) });
}

/// A worktree branch due a poll.
struct Due {
    repository_id: String,
    path: PathBuf,
    branch: String,
    base: String,
}

/// One tick: poll every worktree whose turn it is, without holding the core while `gh` runs.
pub(crate) fn poll(core: &Arc<Mutex<Inner>>) {
    let (forge, due) = {
        let mut inner = core.lock().expect("core lock");
        if inner.forge.setup.is_none() {
            detect(&mut inner);
        }
        if !matches!(inner.forge.setup, Some(ForgeSetup { state: ForgeSetupState::Ready { .. }, .. })) {
            return;
        }
        (inner.runtime.forge.clone(), due(&mut inner))
    };
    let results: Vec<_> = due
        .into_iter()
        .map(|d| {
            let found = forge.pull_request(&d.path, &d.branch);
            (d, found)
        })
        .collect();
    let mut inner = core.lock().expect("core lock");
    for (d, found) in results {
        polled(&mut inner, d, found);
    }
    sessions::flush(&mut inner);
}

/// The worktree branches to poll on this tick; counts down the others. Forgets worktrees
/// that are gone.
fn due(inner: &mut Inner) -> Vec<Due> {
    let mut list = Vec::new();
    let mut seen = HashSet::new();
    let repos = inner.store.repositories().unwrap_or_default();
    for repo in repos {
        let dir = Path::new(&repo.path);
        if git::remote(dir).is_none() {
            continue;
        }
        let Ok(worktrees) = git::worktrees(dir) else { continue };
        let Some(base) = worktrees.first().map(|p| git::base_branch(dir, p)) else { continue };
        for wt in worktrees.into_iter().filter(|w| !w.primary) {
            let Some(branch) = wt.branch else { continue };
            let key = wt.path.to_string_lossy().into_owned();
            seen.insert(key.clone());
            let track = inner.forge.tracks.entry(key).or_default();
            if track.stopped {
                continue;
            }
            if track.wait > 0 {
                track.wait -= 1;
                continue;
            }
            list.push(Due { repository_id: repo.id.clone(), path: wt.path, branch, base: base.clone() });
        }
    }
    inner.forge.tracks.retain(|path, _| seen.contains(path));
    list
}

/// A closed or merged pull request belongs to the worktree only when everything the
/// worktree has was in it (its HEAD is the pull request's head or an ancestor of it): a
/// branch name reused after an old pull request merged is new work, never cleaned up.
fn belongs(path: &Path, found: &ForgePullRequest) -> bool {
    found.pull_request.state == PullRequestState::Open
        || (!found.head_oid.is_empty() && git::is_ancestor(path, "HEAD", &found.head_oid))
}

fn polled(inner: &mut Inner, due: Due, found: Result<Option<ForgePullRequest>, String>) {
    let key = due.path.to_string_lossy().into_owned();
    let Some(track) = inner.forge.tracks.get_mut(&key) else { return };
    let Ok(found) = found else {
        // The Forge could not answer (offline, say): try again later.
        return track.polled(false);
    };
    let found = found.filter(|f| belongs(&due.path, f));
    let before = track.pull_request.as_ref().map(|p| &p.pull_request);
    let changed = before != found.as_ref().map(|p| &p.pull_request);
    track.polled(changed);
    track.pull_request = found.clone();
    if !changed {
        return;
    }
    let pull_request = found.as_ref().map(|f| f.pull_request.clone());
    inner.emit(Event::PullRequestChanged { worktree_path: key.clone(), pull_request: pull_request.clone() });
    match pull_request.map(|p| (p.state, p.number)) {
        Some((PullRequestState::Closed, _)) => track_mut(inner, &key).stopped = true,
        Some((PullRequestState::Merged, number)) => {
            track_mut(inner, &key).stopped = true;
            merged(inner, &due, Some(number));
        }
        _ => {}
    }
}

fn track_mut<'a>(inner: &'a mut Inner, key: &str) -> &'a mut Track {
    inner.forge.tracks.entry(key.to_string()).or_default()
}

/// The branch is in base: clean the worktree up if it is free and clean, and say so.
fn merged(inner: &mut Inner, due: &Due, pull_request: Option<u32>) {
    let path = due.path.to_string_lossy().into_owned();
    let result = worktrees::try_remove(inner, &path);
    // Tasks (M4) move to Done here.
    inner.emit(Event::BranchMerged {
        repository_id: due.repository_id.clone(),
        worktree_path: path,
        branch: due.branch.clone(),
        base: due.base.clone(),
        pull_request,
        removed: result.is_ok(),
        kept: result.err(),
    });
}

/// [`Command::PruneMerged`](switchyard_schema::Command::PruneMerged): asks the Forge about
/// every free, clean worktree of the repository and removes the Merged ones.
pub(crate) fn prune(inner: &mut Inner, repository_id: &str) {
    const COMMAND: &str = "pruneMerged";
    let Some(repo) = worktrees::repository(inner, repository_id) else {
        return inner.refuse(COMMAND, format!("There is no repository {repository_id}"));
    };
    let dir = Path::new(&repo.path);
    if git::remote(dir).is_none() {
        return inner.refuse(COMMAND, "This repository has no remote; nothing is merged on a Forge".into());
    }
    if let Err(reason) = admit(inner) {
        return inner.refuse(COMMAND, reason);
    }
    let list = git::worktrees(dir).unwrap_or_default();
    let forge = inner.runtime.forge.clone();
    let mut branches = Vec::new();
    for wt in list.into_iter().filter(|w| !w.primary) {
        let Some(branch) = wt.branch else { continue };
        let path = wt.path.to_string_lossy().into_owned();
        let at_work = matches!(sessions::occupant(inner, &path), Some(SessionStatus::Running | SessionStatus::Waiting));
        if at_work || !matches!(git::dirty_count(&wt.path), Ok(0)) {
            continue;
        }
        let Ok(Some(found)) = forge.pull_request(&wt.path, &branch) else { continue };
        if found.pull_request.state != PullRequestState::Merged || !belongs(&wt.path, &found) {
            continue;
        }
        if worktrees::try_remove(inner, &path).is_ok() {
            branches.push(branch);
        }
    }
    inner.emit(Event::MergedPruned { repository_id: repo.id, branches });
}

/// [`Command::LocalMerge`](switchyard_schema::Command::LocalMerge): only for a repository
/// with no remote. Merges into base in the Primary worktree, then cleans the worktree up.
pub(crate) fn local_merge(inner: &mut Inner, worktree_path: &str) {
    const COMMAND: &str = "localMerge";
    let Some((repo, worktree)) = inner.find_worktree(worktree_path) else {
        return inner.refuse(COMMAND, format!("{worktree_path} is not a worktree of an added repository"));
    };
    let dir = Path::new(&repo.path);
    if git::remote(dir).is_some() {
        return inner.refuse(COMMAND, "This repository has a remote: work leaves through a pull request, never a local merge".into());
    }
    let Some(branch) = worktree.branch.clone() else {
        return inner.refuse(COMMAND, format!("{worktree_path} has no branch checked out (detached HEAD)"));
    };
    if worktree.primary {
        return inner.refuse(COMMAND, format!("{branch} is the primary worktree; merge a branch worktree into it"));
    }
    let Some(primary) = git::worktrees(dir).ok().and_then(|l| l.into_iter().next()) else {
        return inner.refuse(COMMAND, format!("{} is no longer a readable git repository", repo.path));
    };
    let base = git::base_branch(dir, &primary);
    if let Some(status) = sessions::occupant(inner, worktree_path) {
        if matches!(status, SessionStatus::Running | SessionStatus::Waiting) {
            let label = sessions::status_label(status);
            return inner.refuse(COMMAND, format!("{branch} has a {label} session; stop it before merging"));
        }
    }
    match git::dirty_count(&worktree.path) {
        Ok(0) => {}
        Ok(n) => {
            let files = plural(n, "uncommitted file", "uncommitted files");
            return inner.refuse(COMMAND, format!("{branch} has {files}; commit them before merging"));
        }
        Err(git::GitError(e)) => return inner.refuse(COMMAND, format!("git could not read {branch}: {e}")),
    }
    if primary.branch.as_deref() != Some(base.as_str()) {
        let on = primary.branch.as_deref().unwrap_or("a detached HEAD");
        return inner.refuse(COMMAND, format!("The primary worktree is on {on}, not {base}; switch it to {base} first"));
    }
    match git::dirty_count(&primary.path) {
        Ok(0) => {}
        Ok(n) => {
            let files = plural(n, "uncommitted file", "uncommitted files");
            return inner.refuse(COMMAND, format!("The primary worktree has {files}; commit them before merging into {base}"));
        }
        Err(git::GitError(e)) => return inner.refuse(COMMAND, format!("git could not read the primary worktree: {e}")),
    }
    let (ahead, _) = git::ahead_behind(&worktree.path, &base).unwrap_or((0, 0));
    if ahead == 0 {
        return inner.refuse(COMMAND, format!("{branch} has no commits above {base}; nothing to merge"));
    }
    let conflicts = git::conflicting_files(&primary.path, &branch).unwrap_or_default();
    if !conflicts.is_empty() {
        return inner.refuse(
            COMMAND,
            format!("Merging {branch} into {base} would conflict in {}; rebase {branch} first", conflicts.join(", ")),
        );
    }
    if let Err(git::GitError(e)) = git::merge_branch(&primary.path, &branch, &format!("Merge branch '{branch}'")) {
        return inner.refuse(COMMAND, format!("Could not merge {branch} into {base}: {e}"));
    }
    let due = Due { repository_id: repo.id, path: worktree.path, branch, base };
    merged(inner, &due, None);
}
