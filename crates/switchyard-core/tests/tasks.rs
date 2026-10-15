//! Tasks through seam 1: local tasks kept per repository, Assign to agent, links between
//! sessions and tasks, the local checklist, and the board moving with the session and the
//! Forge (In progress on start, In review on Open pull request, Done on Merged).

mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, git, started_session, Harness, TempRepo};
use switchyard_schema::{
    AgentKind, ChecklistItem, Command, Event, PermissionMode, Task, TaskColumn, TaskKind, TaskSessionTarget,
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

fn links(events: &[Event]) -> Vec<(String, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TaskLinked { session_id, task_id } => Some((session_id.clone(), task_id.clone())),
            _ => None,
        })
        .collect()
}

struct Board {
    repo: TempRepo,
    core: Harness,
    repo_id: String,
}

fn board() -> Board {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    core.send(Command::Attach);
    let repo_id = added_repository(&core.add_repository(&repo.path)).id;
    Board { repo, core, repo_id }
}

impl Board {
    fn create(&mut self, title: &str, checklist: &[&str]) -> Task {
        let events = self.core.send(Command::CreateTask {
            repository_id: self.repo_id.clone(),
            title: title.into(),
            description: format!("About {title}."),
            kind: TaskKind::Feat,
            checklist: checklist.iter().map(|s| s.to_string()).collect(),
        });
        events
            .iter()
            .find_map(|e| match e {
                Event::TaskCreated { task } => Some(task.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected TaskCreated in {events:#?}"))
    }

    fn read(&mut self) -> Vec<Task> {
        let id = self.repo_id.clone();
        tasks_in(&self.core.send(Command::ReadTasks { repository_id: id.clone() }), &id)
    }

    fn column_of(&mut self, task: &Task) -> TaskColumn {
        self.read().into_iter().find(|t| t.id == task.id).expect("task listed").column
    }

    fn assign(&mut self, task: &Task, target: TaskSessionTarget) -> Vec<Event> {
        self.core.send(Command::StartTaskSession {
            task_id: task.id.clone(),
            target,
            agent: AgentKind::ClaudeCode,
            permission_mode: PermissionMode::AskFirst,
            prompt: format!("Work on {}: {}.", task.key, task.title),
        })
    }
}

#[test]
fn a_new_local_task_lands_in_to_do_and_outlives_a_relaunch() {
    let mut b = board();
    assert_eq!(b.read(), vec![], "a repository starts with no tasks");

    let task = b.create("Cursor pagination on orders", &["Cursor param", "OpenAPI"]);
    assert_eq!(task.repository_id, b.repo_id);
    assert_eq!(task.source, TaskSourceKind::Local);
    assert_eq!(task.key, "L-1");
    assert_eq!(task.title, "Cursor pagination on orders");
    assert_eq!(task.kind, TaskKind::Feat);
    assert_eq!(task.column, TaskColumn::Todo);
    assert_eq!(
        task.checklist,
        vec![
            ChecklistItem { text: "Cursor param".into(), done: false },
            ChecklistItem { text: "OpenAPI".into(), done: false },
        ]
    );
    let second = b.create("Rate limits", &[]);
    assert_eq!(second.key, "L-2");
    assert_eq!(b.read(), vec![task.clone(), second.clone()]);

    let mut core = b.core.relaunch();
    core.send(Command::Attach);
    let id = b.repo_id.clone();
    assert_eq!(tasks_in(&core.send(Command::ReadTasks { repository_id: id.clone() }), &id), vec![task, second]);
}

#[test]
fn a_task_needs_a_title_and_a_known_repository() {
    let mut b = board();
    let events = b.core.send(Command::CreateTask {
        repository_id: b.repo_id.clone(),
        title: "  ".into(),
        description: String::new(),
        kind: TaskKind::Fix,
        checklist: vec![],
    });
    assert_eq!(refusal(&events), "Give the task a title");
    let events = b.core.send(Command::ReadTasks { repository_id: "repo-99".into() });
    assert_eq!(refusal(&events), "There is no repository repo-99");
}

#[test]
fn house_rules_come_from_the_repository_policy_or_a_default() {
    let mut b = board();
    let rules = |events: &[Event]| {
        events
            .iter()
            .find_map(|e| match e {
                Event::TasksChanged { house_rules, .. } => Some(house_rules.clone()),
                _ => None,
            })
            .unwrap()
    };
    let id = b.repo_id.clone();
    let default = rules(&b.core.send(Command::ReadTasks { repository_id: id.clone() }));
    assert!(default.contains("Write a failing test first"), "{default}");

    std::fs::create_dir_all(b.repo.path.join(".switchyard")).unwrap();
    std::fs::write(b.repo.path.join(".switchyard/policy.yml"), "house_rules: |\n  Use pnpm, never npm.\n").unwrap();
    assert_eq!(rules(&b.core.send(Command::ReadTasks { repository_id: id })), "Use pnpm, never npm.");
}

#[test]
fn assign_to_agent_starts_a_session_on_a_new_worktree_links_it_and_moves_the_task_to_in_progress() {
    let mut b = board();
    let task = b.create("Cursor pagination on orders", &[]);
    let events = b.assign(&task, TaskSessionTarget::NewWorktree { branch: "feat/l1-cursor-pagination".into() });
    let session = started_session(&events);
    assert_eq!(links(&events), vec![(session.clone(), Some(task.id.clone()))]);
    assert_eq!(tasks_in(&events, &b.repo_id)[0].column, TaskColumn::InProgress);
    let launched = b.core.agents.launches();
    assert_eq!(launched.len(), 1);
    assert!(launched[0].cwd.to_string_lossy().contains("l1-cursor-pagination"), "{:?}", launched[0].cwd);

    // The link and the column outlive a relaunch; attach replays the link.
    let mut core = b.core.relaunch();
    let events = core.send(Command::Attach);
    assert_eq!(links(&events), vec![(session, Some(task.id.clone()))]);
    let id = b.repo_id.clone();
    assert_eq!(tasks_in(&core.send(Command::ReadTasks { repository_id: id.clone() }), &id)[0].column, TaskColumn::InProgress);
}

#[test]
fn assign_to_agent_on_an_existing_worktree_or_replacing_a_paused_session() {
    let mut b = board();
    let wt = b.repo.add_worktree("feat/orders");
    b.core.send(Command::RefreshWorktrees);
    let first = b.create("Orders", &[]);
    let events = b.assign(&first, TaskSessionTarget::Worktree { worktree_path: path(&wt) });
    let s1 = started_session(&events);
    assert_eq!(links(&events), vec![(s1.clone(), Some(first.id.clone()))]);

    b.core.send(Command::StopSession { session_id: s1.clone() });
    let second = b.create("Orders, again", &[]);
    let events = b.assign(&second, TaskSessionTarget::Replace { session_id: s1.clone() });
    let s2 = started_session(&events);
    assert_ne!(s1, s2);
    assert_eq!(links(&events), vec![(s2, Some(second.id.clone()))]);
}

#[test]
fn a_session_that_cannot_start_links_nothing_and_moves_nothing() {
    let mut b = board();
    let task = b.create("Orders", &[]);
    b.core.agents.refuse_next_launch("boom");
    let wt = b.repo.add_worktree("feat/orders");
    let events = b.assign(&task, TaskSessionTarget::Worktree { worktree_path: path(&wt) });
    assert!(refusal(&events).contains("boom"), "{events:#?}");
    assert!(links(&events).is_empty());
    assert_eq!(b.column_of(&task), TaskColumn::Todo);

    let events = b.core.send(Command::StartTaskSession {
        task_id: "nope".into(),
        target: TaskSessionTarget::Worktree { worktree_path: path(&wt) },
        agent: AgentKind::ClaudeCode,
        permission_mode: PermissionMode::AskFirst,
        prompt: "x".into(),
    });
    assert_eq!(refusal(&events), "There is no task nope");
    assert!(b.core.agents.launches().is_empty(), "nothing starts for an unknown task");
}

#[test]
fn a_session_links_to_at_most_one_task_and_relinking_is_explicit() {
    let mut b = board();
    let wt = b.repo.add_worktree("feat/orders");
    let session = started_session(&b.core.start_session(&wt, "Do the orders work."));
    let first = b.create("Orders", &[]);
    let second = b.create("Pagination", &[]);

    let link = |core: &mut Harness, task: Option<&Task>, relink: bool| {
        core.send(Command::LinkTask {
            session_id: session.clone(),
            task_id: task.map(|t| t.id.clone()),
            relink,
        })
    };
    // "Pick one": linking a task to a running session moves it to In progress.
    let events = link(&mut b.core, Some(&first), false);
    assert_eq!(links(&events), vec![(session.clone(), Some(first.id.clone()))]);
    assert_eq!(tasks_in(&events, &b.repo_id)[0].column, TaskColumn::InProgress);

    let events = link(&mut b.core, Some(&second), false);
    assert_eq!(refusal(&events), "feat/orders is already linked to L-1; relink it to change that");
    assert!(links(&events).is_empty());
    assert_eq!(b.column_of(&second), TaskColumn::Todo);

    let events = link(&mut b.core, Some(&second), true);
    assert_eq!(links(&events), vec![(session.clone(), Some(second.id.clone()))]);

    let events = link(&mut b.core, None, false);
    assert_eq!(links(&events), vec![(session.clone(), None)]);
    let events = b.core.send(Command::Attach);
    assert!(links(&events).is_empty(), "an unlinked session replays no link");

    let events = b.core.send(Command::LinkTask { session_id: "session-9".into(), task_id: Some(first.id.clone()), relink: false });
    assert_eq!(refusal(&events), "There is no session session-9");
    let events = link(&mut b.core, Some(&Task { id: "nope".into(), ..first.clone() }), true);
    assert_eq!(refusal(&events), "There is no task nope");
}

#[test]
fn checklist_ticks_persist_in_the_store_and_never_reach_the_task_file() {
    let mut b = board();
    let task = b.create("Orders", &["Cursor param", "OpenAPI"]);
    let files_before = snapshot(&b.core.data_dir().join("tasks"));
    assert!(!files_before.is_empty(), "local tasks live in a JSON file per repository");

    let events = b.core.send(Command::TickTask { task_id: task.id.clone(), item: 1, done: true });
    let ticked = tasks_in(&events, &b.repo_id).into_iter().next().unwrap();
    assert_eq!(
        ticked.checklist,
        vec![
            ChecklistItem { text: "Cursor param".into(), done: false },
            ChecklistItem { text: "OpenAPI".into(), done: true },
        ]
    );
    assert_eq!(snapshot(&b.core.data_dir().join("tasks")), files_before, "ticks stay out of the task file");

    let mut core = b.core.relaunch();
    core.send(Command::Attach);
    let id = b.repo_id.clone();
    let events = core.send(Command::ReadTasks { repository_id: id.clone() });
    assert!(tasks_in(&events, &id)[0].checklist[1].done);

    let events = core.send(Command::TickTask { task_id: task.id.clone(), item: 5, done: true });
    assert_eq!(refusal(&events), "L-1 has no checklist item 6");
}

fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| (e.path(), std::fs::read(e.path()).unwrap())).collect())
        .unwrap_or_default();
    out.sort();
    out
}

/// A board on a repository whose `origin` is a local bare repository, with a signed-in
/// fake `gh`, and a task assigned on `feat/l1-orders` with one commit to ship.
struct Shipping {
    b: Board,
    _bare: tempfile::TempDir,
    task: Task,
    wt: PathBuf,
}

fn shipping() -> Shipping {
    let mut b = board();
    git(&b.repo.path, &["config", "user.name", "Test"]);
    git(&b.repo.path, &["config", "user.email", "test@example.com"]);
    let bare = tempfile::TempDir::new().unwrap();
    git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(&b.repo.path, &["remote", "add", "origin", bare.path().to_str().unwrap()]);
    git(&b.repo.path, &["push", "-q", "-u", "origin", "main"]);
    b.core.gh.install();
    b.core.send(Command::CheckForge);
    let task = b.create("Orders", &[]);
    let events = b.assign(&task, TaskSessionTarget::NewWorktree { branch: "feat/l1-orders".into() });
    let wt = events
        .iter()
        .find_map(|e| match e {
            Event::WorktreeAdded { worktree, .. } => Some(PathBuf::from(&worktree.path)),
            _ => None,
        })
        .unwrap();
    std::fs::write(wt.join("orders.txt"), "orders\n").unwrap();
    git(&wt, &["add", "orders.txt"]);
    git(&wt, &["commit", "-q", "-m", "Orders"]);
    b.core.agents_done();
    Shipping { b, _bare: bare, task, wt }
}

trait Done {
    fn agents_done(&mut self);
}
impl Done for Harness {
    fn agents_done(&mut self) {
        self.agent_says(0, adapter_protocol::Event::Done { summary: None });
    }
}

#[test]
fn open_pull_request_moves_the_task_to_in_review_and_merged_moves_it_to_done() {
    let mut s = shipping();
    let events = s.b.core.send(Command::OpenPullRequest { worktree_path: path(&s.wt) });
    assert!(events.iter().any(|e| matches!(e, Event::PullRequestOpened { .. })), "{events:#?}");
    assert_eq!(tasks_in(&events, &s.b.repo_id)[0].column, TaskColumn::InReview);

    s.b.core.gh.set_state("feat/l1-orders", "MERGED");
    let events = s.b.core.poll_forge();
    assert!(events.iter().any(|e| matches!(e, Event::BranchMerged { removed: true, .. })), "{events:#?}");
    assert_eq!(tasks_in(&events, &s.b.repo_id)[0].column, TaskColumn::Done);
    // The session was archived with the worktree; its link stays.
    let events = s.b.core.send(Command::Attach);
    assert_eq!(links(&events).iter().map(|(_, t)| t.clone()).collect::<Vec<_>>(), vec![Some(s.task.id.clone())]);
}

#[test]
fn a_pull_request_opened_and_merged_outside_switchyard_moves_the_task_too() {
    let mut s = shipping();
    git(&s.wt, &["push", "-q", "origin", "feat/l1-orders"]);
    let head = {
        let out = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&s.wt).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    s.b.core.gh.set_prs("feat/l1-orders", serde_json::json!([harness::gh::gh_pr(7, "OPEN", &head)]));
    let events = s.b.core.poll_forge();
    assert_eq!(tasks_in(&events, &s.b.repo_id)[0].column, TaskColumn::InReview);

    s.b.core.gh.set_state("feat/l1-orders", "MERGED");
    let events = s.b.core.poll_forge();
    assert_eq!(tasks_in(&events, &s.b.repo_id)[0].column, TaskColumn::Done);
}

#[test]
fn a_task_only_moves_forward() {
    let mut s = shipping();
    s.b.core.send(Command::OpenPullRequest { worktree_path: path(&s.wt) });
    assert_eq!(s.b.column_of(&s.task), TaskColumn::InReview);
    // Linking another session to it later does not pull it back to In progress.
    let other = s.b.repo.add_worktree("feat/other");
    let session = started_session(&s.b.core.start_session(&other, "More orders."));
    let events = s.b.core.send(Command::LinkTask { session_id: session, task_id: Some(s.task.id.clone()), relink: false });
    assert!(!events.iter().any(|e| matches!(e, Event::CommandRefused { .. })), "{events:#?}");
    assert_eq!(s.b.column_of(&s.task), TaskColumn::InReview);
}

#[test]
fn a_done_task_cannot_be_assigned_again() {
    let mut s = shipping();
    s.b.core.send(Command::OpenPullRequest { worktree_path: path(&s.wt) });
    s.b.core.gh.set_state("feat/l1-orders", "MERGED");
    s.b.core.poll_forge();
    let task = s.task.clone();
    let events = s.b.assign(&task, TaskSessionTarget::NewWorktree { branch: "feat/l1-again".into() });
    assert_eq!(refusal(&events), "L-1 is done");
}
