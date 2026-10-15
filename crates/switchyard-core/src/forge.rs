//! The Forge (ADR-0003): work leaves a worktree through a pull request. Open pull request
//! pushes the branch and opens one; every worktree branch of a Repository with a remote is
//! then polled on the Forge until its pull request is Merged, whether or not it was opened
//! from Switchyard (a closed one only every [`CLOSED_TICKS`], so a reopen is still seen).
//! Merged cleans the worktree up (removed, shells closed, Session archived); a worktree
//! kept because an agent is at work or files are uncommitted is cleaned up on the first
//! tick after that is no longer so. Prune merged does the same for every free, clean, Merged worktree,
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

use switchyard_schema::command_name;
use switchyard_schema::{
    ChecksState, Event, ForgeSetup, ForgeSetupState, Link, PullRequest, PullRequestState, ReviewState,
    SessionStatus,
};

pub use gh::GhForge;

use crate::{git, sessions, tasks, worktrees, Inner};

/// The Forge as the core uses it. The app uses [`GhForge`]; tests run it against a fake `gh`.
pub trait Forge: Send + Sync {
    /// Is the CLI installed and signed in?
    fn detect(&self) -> ForgeSetup;
    /// Open a pull request from `branch` (already pushed) into `base`, from the worktree at
    /// `dir`, with `text` as its title and body (none: from the commits, as the Forge
    /// fills it). Returns its address.
    fn create_pull_request(&self, dir: &Path, base: &str, branch: &str, text: Option<&PullRequestText>) -> Result<String, String>;
    /// The pull request for `branch` (an open one if any, else the newest), if there is one.
    fn pull_request(&self, dir: &Path, branch: &str) -> Result<Option<ForgePullRequest>, String>;
    /// The repository's issues (open and recently closed), newest first, read from the
    /// worktree at `dir`. Only the GitHub Issues TaskSource asks.
    fn issues(&self, _dir: &Path) -> Result<Vec<ForgeIssue>, String> {
        Err("no Forge is configured".into())
    }
}

/// A pull request's title and body, when Switchyard writes them (to say which linked
/// issues it closes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestText {
    pub title: String,
    pub body: String,
}

/// An issue as the Forge reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForgeIssue {
    pub number: u32,
    pub title: String,
    pub body: String,
    pub url: String,
    pub open: bool,
    pub labels: Vec<String>,
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
    fn create_pull_request(&self, _: &Path, _: &str, _: &str, _: Option<&PullRequestText>) -> Result<String, String> {
        Err("no Forge is configured".into())
    }
    fn pull_request(&self, _: &Path, _: &str) -> Result<Option<ForgePullRequest>, String> {
        Err("no Forge is configured".into())
    }
}

/// The most ticks between two polls of one worktree.
pub const MAX_TICKS: u32 = 8;

/// Ticks between polls of a closed pull request: rare, but a reopen (or a new one opened
/// outside Switchyard) is still seen.
pub const CLOSED_TICKS: u32 = 32;

#[derive(Default)]
pub(crate) struct Tracker {
    setup: Option<ForgeSetup>,
    /// By worktree path.
    tracks: HashMap<String, Track>,
    /// Worktrees whose pull request is being opened right now (the push and `gh` run
    /// without holding the core).
    opening: HashSet<String>,
    /// GitHub issues, by repository id: of repositories with a remote that were synced.
    issues: HashMap<String, Schedule>,
}

/// When a GitHub issues source is next read: the same back-off as a pull request's.
struct Schedule {
    /// Ticks between polls now, and ticks left until the next one.
    every: u32,
    wait: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule { every: 1, wait: 0 }
    }
}

impl Schedule {
    fn polled(&mut self, changed: bool) {
        self.every = if changed { 1 } else { (self.every * 2).min(MAX_TICKS) };
        self.wait = self.every - 1;
    }
}

struct Track {
    pull_request: Option<ForgePullRequest>,
    /// When the Forge was last asked about it (ms since the Unix epoch).
    checked_at_ms: u64,
    /// Ticks between polls now, and ticks left until the next one.
    every: u32,
    wait: u32,
    /// Merged, but the worktree was kept (an agent at work, uncommitted files): the Forge
    /// is not asked again; each tick tries the cleanup again until it succeeds.
    cleanup: Option<Cleanup>,
}

/// A Merged worktree waiting to be cleaned up.
struct Cleanup {
    repository_id: String,
    branch: String,
    base: String,
    pull_request: Option<u32>,
}

impl Default for Track {
    fn default() -> Self {
        Track { pull_request: None, checked_at_ms: 0, every: 1, wait: 0, cleanup: None }
    }
}

impl Track {
    fn polled(&mut self, changed: bool) {
        let closed = self.pull_request.as_ref().is_some_and(|p| p.pull_request.state == PullRequestState::Closed);
        self.every = if closed {
            CLOSED_TICKS
        } else if changed {
            1
        } else {
            (self.every * 2).min(MAX_TICKS)
        };
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
        .filter_map(|(path, t)| t.pull_request.as_ref().map(|p| (path.clone(), p.pull_request.clone(), t.checked_at_ms)))
        .collect();
    known.sort_by(|a, b| a.0.cmp(&b.0));
    for (worktree_path, pull_request, checked_at_ms) in known {
        inner.emit(Event::PullRequestChanged { worktree_path, pull_request: Some(pull_request), checked_at_ms });
    }
}

/// Detect the Forge without holding the core: `gh auth status` may wait on the network.
fn detect_unlocked(core: &Arc<Mutex<Inner>>) -> ForgeSetup {
    let forge = core.lock().expect("core lock").runtime.forge.clone();
    forge.detect()
}

/// Detect the Forge (without holding the core) unless it has been already: before attach
/// and the first poll.
pub(crate) fn detect_if_unknown(core: &Arc<Mutex<Inner>>) {
    if core.lock().expect("core lock").forge.setup.is_some() {
        return;
    }
    let setup = detect_unlocked(core);
    let mut inner = core.lock().expect("core lock");
    if inner.forge.setup.is_none() {
        inner.forge.setup = Some(setup);
    }
}

/// [`Command::CheckForge`](switchyard_schema::Command::CheckForge).
pub(crate) fn check(core: &Arc<Mutex<Inner>>) {
    let forge = detect_unlocked(core);
    let mut inner = core.lock().expect("core lock");
    inner.forge.setup = Some(forge.clone());
    inner.emit(Event::ForgeChecked { forge });
}

fn detect(inner: &mut Inner) {
    inner.forge.setup = Some(inner.runtime.forge.detect());
}

/// Can the Forge be used? Detects again every time (the user may have installed, signed in
/// or signed out since) without holding the core, announcing a change. `Err` is the
/// reason for the user.
pub(crate) fn admit(core: &Arc<Mutex<Inner>>) -> Result<(), String> {
    let setup = detect_unlocked(core);
    let mut inner = core.lock().expect("core lock");
    if inner.forge.setup.as_ref() != Some(&setup) {
        inner.emit(Event::ForgeChecked { forge: setup.clone() });
    }
    inner.forge.setup = Some(setup.clone());
    match setup.state {
        ForgeSetupState::Ready { .. } => Ok(()),
        ForgeSetupState::Missing { install_hint, .. } => {
            Err(format!("GitHub CLI (gh) is not installed; install it ({install_hint}) to open pull requests"))
        }
        ForgeSetupState::SignedOut { login_hint } => {
            Err(format!("GitHub CLI (gh) is not signed in; run {login_hint}, then try again"))
        }
    }
}

/// Does the worktree at `path` have an open pull request, as last seen?
pub(crate) fn has_open_pull_request(inner: &Inner, path: &str) -> bool {
    inner
        .forge
        .tracks
        .get(path)
        .and_then(|t| t.pull_request.as_ref())
        .is_some_and(|p| p.pull_request.state == PullRequestState::Open)
}

/// The repository's GitHub issues were just synced: poll them from now on, starting afresh
/// (every tick, backing off while nothing changes).
pub(crate) fn issues_synced(inner: &mut Inner, repository_id: &str) {
    inner.forge.issues.insert(repository_id.to_string(), Schedule::default());
}

/// The repository's GitHub issues were read again (`changed`: they differ from last time),
/// or could not be (`false`).
pub(crate) fn issues_polled(inner: &mut Inner, repository_id: &str, changed: bool) {
    inner.forge.issues.entry(repository_id.to_string()).or_default().polled(changed);
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

/// "N uncommitted file(s)" and the pronoun that goes with it.
fn uncommitted(n: u32) -> (String, &'static str) {
    (plural(n, "uncommitted file", "uncommitted files"), if n == 1 { "it" } else { "them" })
}

const OPEN_PULL_REQUEST: &str = command_name::OPEN_PULL_REQUEST;

/// What opening a pull request needs, read while holding the core.
struct Opening {
    path: PathBuf,
    branch: String,
    remote: String,
    base: String,
    /// Title and body naming the linked issues it closes; none when nothing is linked.
    text: Option<PullRequestText>,
}

/// For a branch whose sessions are linked to GitHub issues: a title and body that close
/// them on merge ("Closes #412"). The title is the only commit's subject, or the branch's
/// name; the body lists the commits, oldest first.
fn closing_text(path: &Path, onto: &str, branch: &str, issues: &[u32]) -> Option<PullRequestText> {
    if issues.is_empty() {
        return None;
    }
    let commits = git::log_above(path, onto, 50).unwrap_or_default();
    let title = match &commits[..] {
        [only] => only.subject.clone(),
        _ => branch.rsplit('/').next().unwrap_or(branch).replace(['-', '_'], " "),
    };
    let mut body: Vec<String> = commits.iter().rev().map(|c| format!("- {}", c.subject)).collect();
    if !body.is_empty() {
        body.push(String::new());
    }
    body.extend(issues.iter().map(|n| format!("Closes #{n}")));
    Some(PullRequestText { title, body: body.join("\n") })
}

/// [`Command::OpenPullRequest`](switchyard_schema::Command::OpenPullRequest): checked while
/// holding the core; the push and `gh` run without it.
pub(crate) fn open_pull_request(core: &Arc<Mutex<Inner>>, worktree_path: &str) {
    let opening = {
        let mut inner = core.lock().expect("core lock");
        match prepare_opening(&inner, worktree_path) {
            Ok(opening) => {
                inner.forge.opening.insert(worktree_path.to_string());
                opening
            }
            Err(reason) => return inner.refuse(OPEN_PULL_REQUEST, reason),
        }
    };
    let opened = open_unlocked(core, &opening);
    let mut inner = core.lock().expect("core lock");
    inner.forge.opening.remove(worktree_path);
    match opened {
        Ok(opened) => {
            let pull_request = opened.pull_request.clone();
            let checked_at_ms = (inner.runtime.now_ms)();
            let track = Track { pull_request: Some(opened), checked_at_ms, ..Track::default() };
            inner.forge.tracks.insert(worktree_path.to_string(), track);
            inner.emit(Event::PullRequestOpened { worktree_path: worktree_path.into(), pull_request: pull_request.clone() });
            inner.emit(Event::PullRequestChanged {
                worktree_path: worktree_path.into(),
                pull_request: Some(pull_request),
                checked_at_ms,
            });
            tasks::pull_request_open(&inner, Some(OPEN_PULL_REQUEST), worktree_path, &opening.branch);
        }
        Err(reason) => inner.refuse(OPEN_PULL_REQUEST, reason),
    }
    sessions::flush(&mut inner);
}

fn prepare_opening(inner: &Inner, worktree_path: &str) -> Result<Opening, String> {
    let Some((repo, worktree)) = inner.find_worktree(worktree_path) else {
        return Err(format!("{worktree_path} is not a worktree of an added repository"));
    };
    let Some(branch) = worktree.branch.clone() else {
        return Err(format!("{worktree_path} has no branch checked out (detached HEAD)"));
    };
    if worktree.primary {
        return Err(format!("{branch} is the primary worktree; open pull requests from a branch worktree"));
    }
    let Some(remote) = git::remote(Path::new(&repo.path)) else {
        return Err(format!("{branch}'s repository has no remote; use Local merge instead"));
    };
    let Some(base) = base_of(Path::new(&repo.path)) else {
        return Err(format!("{} is no longer a readable git repository", repo.path));
    };
    match git::dirty_count(&worktree.path) {
        Ok(0) => {}
        Ok(n) => {
            let (files, them) = uncommitted(n);
            return Err(format!("{branch} has {files}; commit {them} before opening a pull request"));
        }
        Err(git::GitError(e)) => return Err(format!("git could not read {branch}: {e}")),
    }
    let onto = git::base_ref(&worktree.path, &base);
    let (ahead, behind) = git::ahead_behind(&worktree.path, &onto).unwrap_or((0, 0));
    if behind > 0 {
        let commits = plural(behind, "commit", "commits");
        return Err(format!("{branch} is {commits} behind {base}; rebase before opening a pull request"));
    }
    if ahead == 0 {
        return Err(format!("{branch} has no commits above {base}; commit the work first"));
    }
    if inner.forge.opening.contains(worktree_path) {
        return Err(format!("A pull request is already being opened for {branch}"));
    }
    if let Some(open) = inner.forge.tracks.get(worktree_path).and_then(|t| t.pull_request.as_ref()) {
        if open.pull_request.state == PullRequestState::Open {
            let n = open.pull_request.number;
            return Err(format!("PR #{n} is already open for {branch}"));
        }
    }
    let issues = tasks::linked_issues(inner, worktree_path, &branch);
    let text = closing_text(&worktree.path, &onto, &branch, &issues);
    Ok(Opening { path: worktree.path, branch, remote, base, text })
}

/// Detect, push and open, without holding the core.
fn open_unlocked(core: &Arc<Mutex<Inner>>, o: &Opening) -> Result<ForgePullRequest, String> {
    admit(core)?;
    let forge = core.lock().expect("core lock").runtime.forge.clone();
    let Opening { path, branch, remote, base, text } = o;
    if let Err(git::GitError(e)) = git::push(path, remote, branch) {
        return Err(format!("Could not push {branch} to {remote}: {e}"));
    }
    let url = forge
        .create_pull_request(path, base, branch, text.as_ref())
        .map_err(|e| format!("{branch} was pushed, but the pull request could not be opened: {e}"))?;
    // Read it back for its checks and review; fall back to what the address says.
    let found = forge.pull_request(path, branch).ok().flatten().filter(|p| p.pull_request.url == url);
    Ok(found.unwrap_or_else(|| ForgePullRequest {
        pull_request: PullRequest {
            number: url.rsplit('/').next().and_then(|n| n.parse().ok()).unwrap_or(0),
            url: url.clone(),
            state: PullRequestState::Open,
            draft: false,
            checks: ChecksState::None,
            check_runs: Vec::new(),
            review: ReviewState::None,
        },
        head_oid: String::new(),
    }))
}

/// A worktree branch due a poll.
struct Due {
    repository_id: String,
    path: PathBuf,
    branch: String,
    base: String,
}

/// A worktree branch of a repository with a remote, as git lists it.
struct Candidate {
    repository_id: String,
    path: PathBuf,
    branch: String,
    base: String,
}

/// One tick: poll every worktree whose turn it is. Git and `gh` run without holding the
/// core; it is held only to pick who is due and to take in the answers.
pub(crate) fn poll(core: &Arc<Mutex<Inner>>) {
    detect_if_unknown(core);
    let (forge, repos) = {
        let inner = core.lock().expect("core lock");
        if !matches!(inner.forge.setup, Some(ForgeSetup { state: ForgeSetupState::Ready { .. }, .. })) {
            return;
        }
        (inner.runtime.forge.clone(), inner.store.repositories().unwrap_or_default())
    };
    let candidates = candidates(&repos);
    let remoted: Vec<String> =
        repos.iter().filter(|r| git::remote(Path::new(&r.path)).is_some()).map(|r| r.id.clone()).collect();
    let (due, issues_due) = {
        let mut inner = core.lock().expect("core lock");
        (due(&mut inner, candidates), issues_due(&mut inner, remoted))
    };
    let results: Vec<_> = due
        .into_iter()
        .map(|d| {
            let found = forge.pull_request(&d.path, &d.branch).map(|f| f.filter(|f| belongs(&d.path, f)));
            (d, found)
        })
        .collect();
    let mut inner = core.lock().expect("core lock");
    retry_cleanups(&mut inner);
    let mut merged_in = Vec::new();
    for (d, found) in results {
        let repository_id = d.repository_id.clone();
        if polled(&mut inner, d, found) && !merged_in.contains(&repository_id) {
            merged_in.push(repository_id);
        }
    }
    sessions::flush(&mut inner);
    drop(inner);
    // Issues whose turn it is, and those of a repository where a merge closed the issues its
    // pull request named, on GitHub: read them again so Done shows (Switchyard never closes
    // them itself).
    let mut read = issues_due;
    for repository_id in merged_in {
        if !read.contains(&repository_id) {
            read.push(repository_id);
        }
    }
    for repository_id in read {
        tasks::refresh_issues(core, &repository_id);
    }
}

/// The repositories (of those with a remote) whose GitHub issues are to be read on this
/// tick; counts down the others. Only repositories synced once are polled; forgets those
/// that are gone.
fn issues_due(inner: &mut Inner, remoted: Vec<String>) -> Vec<String> {
    let synced: Vec<String> = remoted.into_iter().filter(|id| tasks::issues_synced(inner, id)).collect();
    inner.forge.issues.retain(|id, _| synced.contains(id));
    let mut list = Vec::new();
    for id in synced {
        let schedule = inner.forge.issues.entry(id.clone()).or_default();
        if schedule.wait > 0 {
            schedule.wait -= 1;
            continue;
        }
        list.push(id);
    }
    list
}

/// Every branch worktree of every repository with a remote (runs git; hold nothing).
fn candidates(repos: &[crate::store::StoredRepository]) -> Vec<Candidate> {
    let mut list = Vec::new();
    for repo in repos {
        let dir = Path::new(&repo.path);
        if git::remote(dir).is_none() {
            continue;
        }
        let Ok(worktrees) = git::worktrees(dir) else { continue };
        let Some(base) = worktrees.first().map(|p| git::base_branch(dir, p)) else { continue };
        for wt in worktrees.into_iter().filter(|w| !w.primary) {
            let Some(branch) = wt.branch else { continue };
            list.push(Candidate { repository_id: repo.id.clone(), path: wt.path, branch, base: base.clone() });
        }
    }
    list
}

/// The candidates to poll on this tick; counts down the others. Forgets worktrees that are
/// gone.
fn due(inner: &mut Inner, candidates: Vec<Candidate>) -> Vec<Due> {
    let mut list = Vec::new();
    let mut seen = HashSet::new();
    for c in candidates {
        let key = c.path.to_string_lossy().into_owned();
        seen.insert(key.clone());
        let track = inner.forge.tracks.entry(key).or_default();
        if track.cleanup.is_some() {
            continue;
        }
        if track.wait > 0 {
            track.wait -= 1;
            continue;
        }
        list.push(Due { repository_id: c.repository_id, path: c.path, branch: c.branch, base: c.base });
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

/// Take in one poll's answer. True when it found the branch Merged.
fn polled(inner: &mut Inner, due: Due, found: Result<Option<ForgePullRequest>, String>) -> bool {
    let key = due.path.to_string_lossy().into_owned();
    let Some(track) = inner.forge.tracks.get_mut(&key) else { return false };
    let Ok(found) = found else {
        // The Forge could not answer (offline, say): try again later.
        track.polled(false);
        return false;
    };
    let before = track.pull_request.as_ref().map(|p| &p.pull_request);
    let changed = before != found.as_ref().map(|p| &p.pull_request);
    let checked_at_ms = (inner.runtime.now_ms)();
    track.pull_request = found.clone();
    track.checked_at_ms = checked_at_ms;
    track.polled(changed);
    let pull_request = found.as_ref().map(|f| f.pull_request.clone());
    if !changed {
        // The same as before, but checked again just now: the card says so.
        if let Some(pull_request) = pull_request {
            inner.emit(Event::PullRequestChanged { worktree_path: key, pull_request: Some(pull_request), checked_at_ms });
        }
        return false;
    }
    inner.emit(Event::PullRequestChanged { worktree_path: key.clone(), pull_request: pull_request.clone(), checked_at_ms });
    match pull_request.map(|p| (p.state, p.number, p.url)) {
        Some((PullRequestState::Merged, number, url)) => {
            merged(inner, None, &due, Some((number, &url)));
            return true;
        }
        // Opened outside Switchyard, or seen again after a relaunch.
        Some((PullRequestState::Open, ..)) => tasks::pull_request_open(inner, None, &key, &due.branch),
        _ => {}
    }
    false
}

/// Each tick: try again to clean up every Merged worktree that was kept, now that its
/// session may have stopped or its files been committed. Quiet until it succeeds.
fn retry_cleanups(inner: &mut Inner) {
    let waiting: Vec<String> =
        inner.forge.tracks.iter().filter(|(_, t)| t.cleanup.is_some()).map(|(path, _)| path.clone()).collect();
    for path in waiting {
        let Some(cleanup) = inner.forge.tracks.get_mut(&path).and_then(|t| t.cleanup.take()) else { continue };
        match worktrees::try_remove(inner, &path) {
            Ok(()) => inner.emit(Event::BranchMerged {
                repository_id: cleanup.repository_id,
                worktree_path: path,
                branch: cleanup.branch,
                base: cleanup.base,
                pull_request: cleanup.pull_request,
                removed: true,
                kept: None,
            }),
            Err(_) => {
                if let Some(track) = inner.forge.tracks.get_mut(&path) {
                    track.cleanup = Some(cleanup);
                }
            }
        }
    }
}

/// The branch is in base: clean the worktree up if it is free and clean, and say so.
/// `pull_request`: its number and address (none for a Local merge). The tasks move first,
/// while the worktree still names its branch's issue.
fn merged(inner: &mut Inner, command: Option<&str>, due: &Due, pull_request: Option<(u32, &str)>) {
    let path = due.path.to_string_lossy().into_owned();
    tasks::merged(inner, command, &path, &due.branch, pull_request.map(|p| p.1));
    let result = worktrees::try_remove(inner, &path);
    if result.is_err() {
        inner.forge.tracks.entry(path.clone()).or_default().cleanup = Some(Cleanup {
            repository_id: due.repository_id.clone(),
            branch: due.branch.clone(),
            base: due.base.clone(),
            pull_request: pull_request.map(|p| p.0),
        });
    }
    inner.emit(Event::BranchMerged {
        repository_id: due.repository_id.clone(),
        worktree_path: path,
        branch: due.branch.clone(),
        base: due.base.clone(),
        pull_request: pull_request.map(|p| p.0),
        removed: result.is_ok(),
        kept: result.err(),
    });
}

/// [`Command::PruneMerged`](switchyard_schema::Command::PruneMerged): asks the Forge about
/// every free, clean worktree of the repository and removes the Merged ones. The Forge is
/// asked without holding the core; who is free and clean is checked again before removing.
pub(crate) fn prune(core: &Arc<Mutex<Inner>>, repository_id: &str) {
    const COMMAND: &str = command_name::PRUNE_MERGED;
    let repo = {
        let inner = core.lock().expect("core lock");
        let Some(repo) = worktrees::repository(&inner, repository_id) else {
            return inner.refuse(COMMAND, crate::no_repository(&repository_id));
        };
        if git::remote(Path::new(&repo.path)).is_none() {
            return inner.refuse(COMMAND, "This repository has no remote; nothing is merged on a Forge".into());
        }
        repo
    };
    if let Err(reason) = admit(core) {
        return core.lock().expect("core lock").refuse(COMMAND, reason);
    }
    let (forge, free) = {
        let inner = core.lock().expect("core lock");
        let list = git::worktrees(Path::new(&repo.path)).unwrap_or_default();
        let free: Vec<_> = list
            .into_iter()
            .filter(|w| !w.primary)
            .filter_map(|w| Some((w.branch.clone()?, w.path)))
            .filter(|(_, path)| is_free_and_clean(&inner, path))
            .collect();
        (inner.runtime.forge.clone(), free)
    };
    let merged: Vec<_> = free
        .into_iter()
        .filter_map(|(branch, path)| {
            let found = forge.pull_request(&path, &branch).ok().flatten()?;
            let merged = found.pull_request.state == PullRequestState::Merged && belongs(&path, &found);
            merged.then_some((branch, path, found.pull_request.url))
        })
        .collect();
    let mut inner = core.lock().expect("core lock");
    let mut branches = Vec::new();
    for (branch, path, url) in merged {
        if !is_free_and_clean(&inner, &path) {
            continue;
        }
        let key = path.to_string_lossy().into_owned();
        tasks::merged(&inner, Some(COMMAND), &key, &branch, Some(&url));
        if worktrees::try_remove(&mut inner, &key).is_ok() {
            branches.push(branch);
        }
    }
    inner.emit(Event::MergedPruned { repository_id: repo.id, branches });
    sessions::flush(&mut inner);
}

/// No agent at work in it and nothing uncommitted.
fn is_free_and_clean(inner: &Inner, path: &Path) -> bool {
    let key = path.to_string_lossy();
    let at_work = matches!(sessions::occupant(inner, &key), Some(SessionStatus::Running | SessionStatus::Waiting));
    !at_work && matches!(git::dirty_count(path), Ok(0))
}

/// [`Command::LocalMerge`](switchyard_schema::Command::LocalMerge): only for a repository
/// with no remote. Merges into base in the Primary worktree, then cleans the worktree up.
pub(crate) fn local_merge(inner: &mut Inner, worktree_path: &str) {
    const COMMAND: &str = command_name::LOCAL_MERGE;
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
            let (files, them) = uncommitted(n);
            return inner.refuse(COMMAND, format!("{branch} has {files}; commit {them} before merging"));
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
            let (files, them) = uncommitted(n);
            return inner.refuse(COMMAND, format!("The primary worktree has {files}; commit {them} before merging into {base}"));
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
    merged(inner, Some(COMMAND), &due, None);
}
