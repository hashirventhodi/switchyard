//! The GitHub Issues TaskSource through seam 1: Sync pulls the repository's issues through
//! the Forge (a fake `gh`, see `harness/gh.rs`; nothing touches GitHub) onto the board
//! beside local tasks, with the lossy column mapping (To do = open, no branch; In progress
//! = open with a linked session; In review = its pull request open; Done = closed on
//! GitHub). Nothing is written to the tracker: a pull request opened for a linked issue
//! says "Closes #N" and GitHub closes it on merge.

mod harness;

use std::path::{Path, PathBuf};

use harness::gh::gh_issue;
use harness::{added_repository, git, Harness, TempRepo, NOW_MS};
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

/// The `--body` a `pr create` call was given, if any.
fn body_of(call: &[String]) -> Option<String> {
    call.iter().position(|a| a == "--body").and_then(|i| call.get(i + 1).cloned())
}

#[test]
fn an_issue_moves_with_its_linked_work_and_is_done_when_github_closes_it() {
    let mut t = tracker();
    t.sync();

    // The branch is not named for the issue: the link is the session's.
    let (events, wt) = t.assign_and_commit("#412", "fix/login-timeout");
    assert_eq!(by_key(&tasks_in(&events, &t.repo_id), "#412").column, TaskColumn::InProgress);
    assert_eq!(t.writes(), Vec::<Vec<String>>::new(), "In progress is not written to GitHub");

    let events = t.core.send(Command::OpenPullRequest { worktree_path: path(&wt) });
    assert!(events.iter().any(|e| matches!(e, Event::PullRequestOpened { .. })), "{events:#?}");
    assert_eq!(by_key(&tasks_in(&events, &t.repo_id), "#412").column, TaskColumn::InReview);
    let writes = t.writes();
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert_eq!(writes[0][..2], ["pr", "create"]);
    let body = body_of(&writes[0]).expect("a body naming the issue");
    assert!(body.ends_with("Closes #412"), "{body}");
    assert!(body.contains("Fix the timeout"), "{body}");

    // GitHub closes the issue when the pull request merges ("Closes #412"); Switchyard
    // never does. The poller reads the issues again once a linked branch is Merged.
    t.core.gh.set_issues(json!([
        gh_issue(412, "Login times out", "The form hangs.", "CLOSED", &["bug"]),
        gh_issue(7, "Old thing", "", "CLOSED", &[]),
    ]));
    t.core.gh.set_state("fix/login-timeout", "MERGED");
    let events = t.core.poll_forge();
    assert!(events.iter().any(|e| matches!(e, Event::BranchMerged { removed: true, .. })), "{events:#?}");
    assert_eq!(by_key(&tasks_in(&events, &t.repo_id), "#412").column, TaskColumn::Done);
    assert_eq!(t.writes().len(), 1, "nothing else is written: {:?}", t.writes());

    // Still Done after a relaunch, before any sync (the store keeps what GitHub said).
    let mut core = t.core.relaunch();
    core.send(Command::Attach);
    let id = t.repo_id.clone();
    let tasks = tasks_in(&core.send(Command::ReadTasks { repository_id: id.clone() }), &id);
    assert_eq!(by_key(&tasks, "#412").column, TaskColumn::Done);
}

#[test]
fn an_issue_closed_by_hand_is_done_on_the_next_sync() {
    let mut t = tracker();
    t.sync();
    t.assign_and_commit("#412", "fix/login-timeout");
    assert_eq!(t.issue("#412").column, TaskColumn::InProgress);
    t.core.gh.set_issues(json!([gh_issue(412, "Login times out", "", "CLOSED", &["bug"])]));
    assert_eq!(by_key(&tasks_in(&t.sync(), &t.repo_id), "#412").column, TaskColumn::Done);
    assert_eq!(t.writes(), Vec::<Vec<String>>::new());
}

#[test]
fn a_pull_request_with_no_linked_issue_closes_nothing() {
    let mut t = tracker();
    t.sync();
    // Named like the issue, but no session is linked to it.
    let wt = t.repo.add_worktree("fix/412-login");
    std::fs::write(wt.join("fix.txt"), "fixed\n").unwrap();
    git(&wt, &["add", "fix.txt"]);
    git(&wt, &["commit", "-q", "-m", "Fix it"]);
    t.core.send(Command::OpenPullRequest { worktree_path: path(&wt) });
    let writes = t.writes();
    assert_eq!(writes[0], vec!["pr", "create", "--base", "main", "--head", "fix/412-login", "--fill"]);
    assert_eq!(t.issue("#412").column, TaskColumn::Todo, "a branch name links nothing");
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

    let synced = tasks_in(&t.sync(), &t.repo_id);
    assert_eq!(synced, tasks_in(&t.sync(), &t.repo_id));
    assert_eq!(t.writes().len(), 1, "only the pull request: {:?}", t.writes());
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
fn a_worktree_branch_named_like_an_issue_does_not_link_it() {
    let mut t = tracker();
    t.sync();
    t.repo.add_worktree("412-login-timeout");
    assert_eq!(t.issue("#412").column, TaskColumn::Todo);
    assert_eq!(t.writes(), Vec::<Vec<String>>::new());
}

#[test]
fn once_synced_a_new_issue_on_github_appears_after_a_forge_tick() {
    let mut t = tracker();
    t.sync();
    t.core.gh.set_issues(json!([
        gh_issue(413, "Signup is slow", "", "OPEN", &[]),
        gh_issue(412, "Login times out", "", "OPEN", &["bug"]),
        gh_issue(7, "Old thing", "", "CLOSED", &[]),
    ]));
    let events = t.core.poll_forge();
    let tasks = tasks_in(&events, &t.repo_id);
    assert_eq!(by_key(&tasks, "#413").column, TaskColumn::Todo);
    assert_eq!(tasks.len(), 4, "{tasks:#?}");
}

/// The `checkedAtMs` of the last task list emitted for the repository.
fn checked_in(events: &[Event], repo_id: &str) -> Option<u64> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::TasksChanged { repository_id, checked_at_ms, .. } if repository_id == repo_id => Some(*checked_at_ms),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected TasksChanged for {repo_id} in {events:#?}"))
}

#[test]
fn the_task_list_says_when_the_issues_were_last_checked() {
    let mut t = tracker();
    let events = t.core.send(Command::ReadTasks { repository_id: t.repo_id.clone() });
    assert_eq!(checked_in(&events, &t.repo_id), None, "never synced");
    assert_eq!(checked_in(&t.sync(), &t.repo_id), Some(NOW_MS));

    // Read again by the poller, later.
    let core = std::mem::replace(&mut t.core, Harness::launch()).relaunch_at(|| NOW_MS + 15_000);
    t.core = core;
    t.core.send(Command::Attach);
    let events = t.core.send(Command::ReadTasks { repository_id: t.repo_id.clone() });
    assert_eq!(checked_in(&events, &t.repo_id), Some(NOW_MS), "kept across a relaunch");
    assert_eq!(checked_in(&t.core.poll_forge(), &t.repo_id), Some(NOW_MS + 15_000));
}

/// The ticks (1-based) on which `issue list` ran, over `ticks` ticks.
fn issues_polled_on(core: &mut Harness, ticks: u32) -> Vec<u32> {
    let mut on = Vec::new();
    for tick in 1..=ticks {
        let before = core.gh.calls_of(["issue", "list"]);
        core.poll_forge();
        if core.gh.calls_of(["issue", "list"]) > before {
            on.push(tick);
        }
    }
    on
}

#[test]
fn issue_polling_backs_off_while_nothing_changes_and_speeds_up_on_a_change() {
    let mut t = tracker();
    t.sync();
    // 1, then every 2, 4, 8 ticks, never further apart than 8.
    assert_eq!(issues_polled_on(&mut t.core, 32), vec![1, 3, 7, 15, 23, 31]);

    t.core.gh.set_issues(json!([gh_issue(412, "Login times out", "", "CLOSED", &["bug"])]));
    // Next due on tick 39 (8 after 31), the 7th of these; it sees the change, so the next
    // poll is on the next tick, then they back off again.
    assert_eq!(issues_polled_on(&mut t.core, 12), vec![7, 8, 10]);
}

#[test]
fn sync_now_reads_at_once_and_resets_the_back_off() {
    let mut t = tracker();
    t.sync();
    assert_eq!(issues_polled_on(&mut t.core, 8), vec![1, 3, 7]);
    let before = t.core.gh.calls_of(["issue", "list"]);
    t.sync();
    assert_eq!(t.core.gh.calls_of(["issue", "list"]), before + 1, "Sync now reads at once");
    assert_eq!(issues_polled_on(&mut t.core, 8), vec![1, 3, 7], "and polls from every tick again");
}

#[test]
fn a_hidden_window_polls_issues_at_the_slowest_cadence_and_showing_it_polls_at_once() {
    let mut t = tracker();
    t.sync();
    t.core.send(Command::WindowVisibility { visible: false });
    assert_eq!(issues_polled_on(&mut t.core, 24), vec![1, 9, 17]);
    let before = t.core.gh.calls_of(["issue", "list"]);
    t.core.send(Command::WindowVisibility { visible: true });
    assert_eq!(t.core.gh.calls_of(["issue", "list"]), before + 1, "polled at once");
    assert_eq!(issues_polled_on(&mut t.core, 8), vec![2, 6]);
}

#[test]
fn poll_forge_now_reads_issues_at_once_and_resets_their_back_off() {
    let mut t = tracker();
    t.sync();
    assert_eq!(issues_polled_on(&mut t.core, 16), vec![1, 3, 7, 15]);
    let before = t.core.gh.calls_of(["issue", "list"]);
    t.core.send(Command::PollForgeNow);
    assert_eq!(t.core.gh.calls_of(["issue", "list"]), before + 1, "read at once");
    assert_eq!(issues_polled_on(&mut t.core, 8), vec![2, 6]);
}

#[test]
fn a_repository_never_synced_is_left_alone_by_the_poller() {
    let mut t = tracker();
    assert_eq!(issues_polled_on(&mut t.core, 16), Vec::<u32>::new());

    // A sync that found no issues still counts as synced.
    t.core.gh.set_issues(json!([]));
    t.sync();
    assert_eq!(issues_polled_on(&mut t.core, 3), vec![1, 3]);
}
