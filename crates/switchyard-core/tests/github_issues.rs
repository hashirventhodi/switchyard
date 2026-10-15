//! The GitHub Issues TaskSource through seam 1: Sync pulls the repository's issues through
//! the Forge (a fake `gh`, see `harness/gh.rs`; nothing touches GitHub) onto the board
//! beside local tasks, with the lossy column mapping (To do = open, no branch; In progress
//! = open with a linked branch; In review = pull request open; Done = closed). Only one
//! thing is ever written back: Merged closes the issue with a comment linking the PR.

mod harness;

use std::path::{Path, PathBuf};

use harness::gh::gh_issue;
use harness::{added_repository, git, Harness, TempRepo};
use serde_json::json;
use switchyard_schema::{
    Command, Event, ForgeSetupState, PermissionMode, AgentKind, Task, TaskColumn, TaskKind, TaskSessionTarget,
    TaskSourceKind,
};

fn path(p: &Path) -> String {
    p.to_str().unwrap().into()
}

/// The last full task list emitted for the repository.
fn tasks_in(events: &[Event], repo_id: &str) -> Vec<Task> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::TasksChanged { repository_id, tasks, .. } if repository_id == repo_id => Some(tasks.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected TasksChanged for {repo_id} in {events:#?}"))
}

fn refusal(events: &[Event]) -> String {
    events
        .iter()
        .find_map(|e| match e {
            Event::CommandRefused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a refusal in {events:#?}"))
}

fn by_key<'a>(tasks: &'a [Task], key: &str) -> &'a Task {
    tasks.iter().find(|t| t.key == key).unwrap_or_else(|| panic!("no task {key} in {tasks:#?}"))
}

/// A repository whose `origin` is a local bare repository, a signed-in fake `gh` with
/// issues #412 (open, labelled bug) and #7 (closed), and one local task.
struct Tracker {
    repo: TempRepo,
    _bare: tempfile::TempDir,
    core: Harness,
    repo_id: String,
}

fn tracker() -> Tracker {
    let repo = TempRepo::new("acme");
    git(&repo.path, &["config", "user.name", "Test"]);
    git(&repo.path, &["config", "user.email", "test@example.com"]);
    let bare = tempfile::TempDir::new().unwrap();
    git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(&repo.path, &["remote", "add", "origin", bare.path().to_str().unwrap()]);
    git(&repo.path, &["push", "-q", "-u", "origin", "main"]);
    let mut core = Harness::launch();
    core.gh.install();
    core.gh.set_issues(json!([
        gh_issue(412, "Login times out", "The form hangs.\n\n- [ ] Reproduce\n- [x] Find the timeout\n", "OPEN", &["bug"]),
        gh_issue(7, "Old thing", "", "CLOSED", &[]),
    ]));
    core.send(Command::Attach);
    let repo_id = added_repository(&core.add_repository(&repo.path)).id;
    core.send(Command::CreateTask {
        repository_id: repo_id.clone(),
        title: "Local chore".into(),
        description: String::new(),
        kind: TaskKind::Chore,
        checklist: vec![],
    });
    Tracker { repo, _bare: bare, core, repo_id }
}

impl Tracker {
    fn sync(&mut self) -> Vec<Event> {
        self.core.send(Command::SyncTasks { repository_id: self.repo_id.clone() })
    }

    fn read(&mut self) -> Vec<Task> {
        let events = self.core.send(Command::ReadTasks { repository_id: self.repo_id.clone() });
        tasks_in(&events, &self.repo_id)
    }

    /// Every `gh` invocation that is not a read.
    fn writes(&self) -> Vec<Vec<String>> {
        let reads = [["--version"].as_slice(), &["auth", "status"], &["issue", "list"], &["pr", "list"]];
        self.core
            .gh
            .calls()
            .into_iter()
            .filter(|c| !reads.iter().any(|r| c.len() >= r.len() && c[..r.len()] == **r))
            .collect()
    }
}

#[test]
fn sync_puts_issues_on_the_board_beside_local_tasks() {
    let mut t = tracker();
    let events = t.sync();
    let tasks = tasks_in(&events, &t.repo_id);
    assert_eq!(tasks.len(), 3, "{tasks:#?}");

    let issue = by_key(&tasks, "#412");
    assert_eq!(issue.source, TaskSourceKind::Github);
    assert_eq!(issue.id, format!("{}/github/#412", t.repo_id));
    assert_eq!(issue.title, "Login times out");
    assert!(issue.description.starts_with("The form hangs."), "{issue:#?}");
    assert_eq!(issue.kind, TaskKind::Fix, "a bug label makes it a fix");
    assert_eq!(issue.column, TaskColumn::Todo);
    assert_eq!(issue.url.as_deref(), Some("https://github.com/acme/acme/issues/412"));
    let items: Vec<_> = issue.checklist.iter().map(|c| (c.text.as_str(), c.done)).collect();
    assert_eq!(items, vec![("Reproduce", false), ("Find the timeout", false)], "ticks are local only");

    assert_eq!(by_key(&tasks, "#7").column, TaskColumn::Done);
    assert_eq!(by_key(&tasks, "L-1").source, TaskSourceKind::Local);
    assert_eq!(t.writes(), Vec::<Vec<String>>::new(), "sync never writes");
}

impl Tracker {
    fn issue(&mut self, key: &str) -> Task {
        by_key(&self.read(), key).clone()
    }

    /// Assign the issue to an agent on a new worktree `branch` with one commit to ship, and
    /// let the agent finish. Returns the worktree.
    fn assign_and_commit(&mut self, key: &str, branch: &str) -> (Vec<Event>, PathBuf) {
        let task = self.issue(key);
        let events = self.core.send(Command::StartTaskSession {
            task_id: task.id.clone(),
            target: TaskSessionTarget::NewWorktree { branch: branch.into() },
            agent: AgentKind::ClaudeCode,
            permission_mode: PermissionMode::AskFirst,
            prompt: format!("Work on {}: {}.", task.key, task.title),
        });
        let wt = events
            .iter()
            .find_map(|e| match e {
                Event::WorktreeAdded { worktree, .. } => Some(PathBuf::from(&worktree.path)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected a worktree in {events:#?}"));
        std::fs::write(wt.join("fix.txt"), "fixed\n").unwrap();
        git(&wt, &["add", "fix.txt"]);
        git(&wt, &["commit", "-q", "-m", "Fix the timeout"]);
        self.core.agent_says(0, adapter_protocol::Event::Done { summary: None });
        (events, wt)
    }
}

#[test]
fn an_issue_moves_with_the_work_and_merged_closes_it_with_a_comment_linking_the_pr() {
    let mut t = tracker();
    t.sync();

    let (events, wt) = t.assign_and_commit("#412", "fix/412-login-times-out");
    assert_eq!(by_key(&tasks_in(&events, &t.repo_id), "#412").column, TaskColumn::InProgress);
    assert_eq!(t.writes(), Vec::<Vec<String>>::new(), "In progress is not written to GitHub");

    let events = t.core.send(Command::OpenPullRequest { worktree_path: path(&wt) });
    assert!(events.iter().any(|e| matches!(e, Event::PullRequestOpened { .. })), "{events:#?}");
    assert_eq!(by_key(&tasks_in(&events, &t.repo_id), "#412").column, TaskColumn::InReview);
    let writes = t.writes();
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert_eq!(writes[0][..2], ["pr", "create"], "In review is not written to the issue");

    t.core.gh.set_state("fix/412-login-times-out", "MERGED");
    let events = t.core.poll_forge();
    assert!(events.iter().any(|e| matches!(e, Event::BranchMerged { removed: true, .. })), "{events:#?}");
    assert_eq!(by_key(&tasks_in(&events, &t.repo_id), "#412").column, TaskColumn::Done);
    assert_eq!(
        t.core.gh.calls_with(["issue", "close"]),
        vec![vec!["issue", "close", "412", "--comment", "Merged in https://github.com/acme/acme/pull/421."]],
    );
    assert_eq!(t.writes().len(), 2, "nothing else is written: {:?}", t.writes());

    // Still Done after a relaunch, before any sync (the store keeps what GitHub said).
    let mut core = t.core.relaunch();
    core.send(Command::Attach);
    let id = t.repo_id.clone();
    let tasks = tasks_in(&core.send(Command::ReadTasks { repository_id: id.clone() }), &id);
    assert_eq!(by_key(&tasks, "#412").column, TaskColumn::Done);
    assert_eq!(by_key(&tasks, "#7").column, TaskColumn::Done);
}

#[test]
fn sync_is_idempotent_and_never_closes_twice() {
    let mut t = tracker();
    let first = tasks_in(&t.sync(), &t.repo_id);
    let again = tasks_in(&t.sync(), &t.repo_id);
    assert_eq!(first, again);

    let (_, wt) = t.assign_and_commit("#412", "fix/412-login-times-out");
    t.core.send(Command::OpenPullRequest { worktree_path: path(&wt) });
    // GitHub closed the issue itself (the PR said "Fixes #412"); a sync sees it first.
    t.core.gh.set_issues(json!([
        gh_issue(412, "Login times out", "The form hangs.", "CLOSED", &["bug"]),
        gh_issue(7, "Old thing", "", "CLOSED", &[]),
    ]));
    assert_eq!(by_key(&tasks_in(&t.sync(), &t.repo_id), "#412").column, TaskColumn::Done);
    t.core.gh.set_state("fix/412-login-times-out", "MERGED");
    let events = t.core.poll_forge();
    assert!(events.iter().any(|e| matches!(e, Event::BranchMerged { .. })), "{events:#?}");
    assert_eq!(t.core.gh.calls_of(["issue", "close"]), 0, "an issue already closed is not closed again");

    let synced = tasks_in(&t.sync(), &t.repo_id);
    assert_eq!(synced, tasks_in(&t.sync(), &t.repo_id));
    assert_eq!(t.core.gh.calls_of(["issue", "close"]), 0);
}

#[test]
fn a_signed_out_or_missing_gh_keeps_the_board_and_names_the_setup_state() {
    let mut t = tracker();
    t.sync();
    t.core.gh.sign_out();
    let events = t.sync();
    assert_eq!(
        refusal(&events),
        "Could not sync GitHub issues: GitHub CLI (gh) is not signed in; run gh auth login, then try again"
    );
    let setup = events.iter().find_map(|e| match e {
        Event::ForgeChecked { forge } => Some(forge.state.clone()),
        _ => None,
    });
    assert_eq!(setup, Some(ForgeSetupState::SignedOut { login_hint: "gh auth login".into() }));
    let tasks = tasks_in(&events, &t.repo_id);
    assert_eq!(tasks.len(), 3, "local tasks and the issues last synced stay: {tasks:#?}");
    assert_eq!(t.core.gh.calls_of(["issue", "list"]), 1, "a signed-out gh is not asked for issues");

    // Never synced and no gh at all: only the local tasks, and the reason.
    let mut core = Harness::launch();
    core.send(Command::Attach);
    let id = added_repository(&core.add_repository(&t.repo.path)).id;
    let events = core.send(Command::SyncTasks { repository_id: id.clone() });
    assert!(refusal(&events).starts_with("Could not sync GitHub issues: GitHub CLI (gh) is not installed"), "{events:#?}");
    assert_eq!(tasks_in(&events, &id), vec![]);
}

#[test]
fn a_repository_without_a_remote_has_only_local_tasks() {
    let repo = TempRepo::new("offline");
    let mut core = Harness::launch();
    core.gh.install();
    core.gh.set_issues(json!([gh_issue(1, "Elsewhere", "", "OPEN", &[])]));
    core.send(Command::Attach);
    let id = added_repository(&core.add_repository(&repo.path)).id;
    let events = core.send(Command::SyncTasks { repository_id: id.clone() });
    assert_eq!(refusal(&events), "offline has no remote, so no GitHub issues; its tasks are local");
    assert_eq!(tasks_in(&events, &id), vec![]);
    assert_eq!(core.gh.calls_of(["issue", "list"]), 0);
}

#[test]
fn a_worktree_branch_named_for_an_issue_links_it() {
    let mut t = tracker();
    t.sync();
    t.repo.add_worktree("412-login-timeout");
    assert_eq!(t.issue("#412").column, TaskColumn::InProgress);
    t.repo.add_worktree("feat/4120-other");
    assert_eq!(t.issue("#7").column, TaskColumn::Done);
    assert_eq!(t.writes(), Vec::<Vec<String>>::new());
}

#[test]
fn a_failed_close_leaves_the_issue_open_until_github_says_otherwise() {
    let mut t = tracker();
    t.sync();
    let (_, wt) = t.assign_and_commit("#412", "fix/412-login-times-out");
    t.core.send(Command::OpenPullRequest { worktree_path: path(&wt) });
    t.core.gh.fail_close("HTTP 403: Resource not accessible by integration");
    t.core.gh.set_state("fix/412-login-times-out", "MERGED");
    let events = t.core.poll_forge();
    assert!(events.iter().any(|e| matches!(e, Event::BranchMerged { removed: true, .. })), "{events:#?}");
    assert_eq!(t.core.gh.calls_of(["issue", "close"]), 1);
    assert_eq!(t.issue("#412").column, TaskColumn::InProgress, "still open on GitHub, its session linked");
}
