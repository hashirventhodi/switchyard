//! Worktree lifecycle through seam 1: create into the Switchyard worktree root, adopt
//! hand-made worktrees in place, remove only clean ones, keep working sessions off primary.
mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, started_session, Harness, TempRepo};
use switchyard_schema::{AgentKind, Command, Event, PermissionMode, Worktree, WorktreeFacts};

fn create(core: &mut Harness, repository_id: &str, branch: &str) -> Vec<Event> {
    core.send(Command::CreateWorktree { repository_id: repository_id.into(), branch: branch.into() })
}

/// The single worktree announced in `events`.
fn added_worktree(events: &[Event]) -> Worktree {
    let list: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorktreeAdded { worktree, .. } => Some(worktree.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(list.len(), 1, "expected exactly one WorktreeAdded in {events:#?}");
    list.into_iter().next().unwrap()
}

fn refused(events: &[Event]) -> String {
    match events {
        [Event::CommandRefused { reason, .. }] => reason.clone(),
        other => panic!("expected one refusal, got {other:#?}"),
    }
}

fn git_worktree_paths(repo: &Path) -> Vec<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("worktree ").map(PathBuf::from))
        .collect()
}

#[test]
fn a_new_worktree_is_created_from_base_under_the_switchyard_root_outside_the_repo() {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));

    let events = create(&mut core, &repository.id, "feat/login");

    let worktree = added_worktree(&events);
    let path = PathBuf::from(&worktree.path);
    assert_eq!(worktree.branch.as_deref(), Some("feat/login"));
    assert!(!worktree.primary);
    assert!(path.starts_with(core.worktree_root()), "{path:?} not under {:?}", core.worktree_root());
    assert!(!path.starts_with(&repo.path));
    assert!(worktree.path.is_ascii() && !worktree.path.contains(' '), "{path:?}");
    assert!(git_worktree_paths(&repo.path).contains(&path));
    assert!(path.join("README").is_file(), "checked out from base");
}

#[test]
fn two_clones_with_the_same_folder_name_get_distinct_worktree_directories() {
    let first = TempRepo::new("acme");
    let second = TempRepo::new("acme");
    let mut core = Harness::launch();
    let a = added_repository(&core.add_repository(&first.path));
    let b = added_repository(&core.add_repository(&second.path));

    let in_a = added_worktree(&create(&mut core, &a.id, "feat/login"));
    let in_b = added_worktree(&create(&mut core, &b.id, "feat/login"));

    assert_ne!(in_a.path, in_b.path);
    assert!(git_worktree_paths(&first.path).contains(&PathBuf::from(&in_a.path)));
    assert!(git_worktree_paths(&second.path).contains(&PathBuf::from(&in_b.path)));
}

#[test]
fn a_repository_policy_can_name_its_own_worktree_root() {
    let repo = TempRepo::new("acme");
    std::fs::create_dir(repo.path.join(".switchyard")).unwrap();
    std::fs::write(
        repo.path.join(".switchyard/policy.yml"),
        "# Our tooling expects worktrees next to the checkout.\nworktrees:\n  root: ../acme-trees\n",
    )
    .unwrap();
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));
    // A committed root is a request (ADR-0006); once acknowledged it is used.
    core.acknowledge_policy(&repository.id);

    let worktree = added_worktree(&create(&mut core, &repository.id, "feat/login"));

    let expected = repo.path.parent().unwrap().join("acme-trees/feat-login");
    assert_eq!(PathBuf::from(&worktree.path), expected);
    assert!(git_worktree_paths(&repo.path).contains(&expected));
}

#[test]
fn an_unreadable_policy_file_refuses_creation_with_the_reason() {
    let repo = TempRepo::new("acme");
    std::fs::create_dir(repo.path.join(".switchyard")).unwrap();
    std::fs::write(repo.path.join(".switchyard/policy.yml"), "worktrees: [not, a, map\n").unwrap();
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));

    let reason = refused(&create(&mut core, &repository.id, "feat/login"));

    assert!(reason.contains("policy.yml"), "{reason}");
    assert_eq!(git_worktree_paths(&repo.path), vec![repo.path.clone()]);
}

#[test]
fn a_worktree_added_by_hand_appears_after_relaunch_at_its_own_path() {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));
    let ours = added_worktree(&create(&mut core, &repository.id, "feat/login"));

    // While the app is closed, the user adds a worktree with git directly.
    let by_hand = repo.add_worktree("spike/cache");
    let mut core = core.relaunch();
    let events = core.send(Command::Attach);

    let mut listed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorktreeAdded { worktree, .. } => Some(PathBuf::from(&worktree.path)),
            _ => None,
        })
        .collect();
    listed.sort();
    let mut expected = vec![repo.path.clone(), PathBuf::from(&ours.path), by_hand.clone()];
    expected.sort();
    assert_eq!(listed, expected);
    assert!(!by_hand.starts_with(core.worktree_root()), "adopted in place, not moved");
    assert!(by_hand.join("README").is_file());
    let refreshed = core.send(Command::RefreshWorktrees);
    assert_eq!(facts_of(&refreshed, &by_hand), WorktreeFacts::default());
}

fn facts_of(events: &[Event], path: &Path) -> WorktreeFacts {
    let found: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorktreeFactsChanged { worktree_path, facts } if Path::new(worktree_path) == path => {
                Some(facts.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(found.len(), 1, "expected one facts event for {path:?} in {events:#?}");
    found.into_iter().next().unwrap()
}

#[test]
fn worktree_facts_count_commits_ahead_and_behind_base_and_uncommitted_files() {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));
    let events = create(&mut core, &repository.id, "feat/login");
    let path = PathBuf::from(added_worktree(&events).path);
    assert_eq!(facts_of(&events, &path), WorktreeFacts { ahead: 0, behind: 0, dirty: 0, conflicts: vec![], changed_files: 0, commits: 0 });

    std::fs::write(path.join("login.rs"), "fn login() {}\n").unwrap();
    harness::git(&path, &["add", "login.rs"]);
    harness::git(&path, &["commit", "-q", "-m", "login"]);
    std::fs::write(path.join("README"), "changed\n").unwrap();
    std::fs::write(path.join("notes.txt"), "scratch\n").unwrap();
    std::fs::write(repo.path.join("CHANGELOG"), "base moved\n").unwrap();
    harness::git(&repo.path, &["add", "CHANGELOG"]);
    harness::git(&repo.path, &["commit", "-q", "-m", "base moved"]);

    let events = core.send(Command::RefreshWorktrees);

    assert_eq!(facts_of(&events, &path), WorktreeFacts { ahead: 1, behind: 1, dirty: 2, conflicts: vec![], changed_files: 2, commits: 1 });
    assert_eq!(facts_of(&events, &repo.path), WorktreeFacts { ahead: 0, behind: 0, dirty: 0, conflicts: vec![], changed_files: 0, commits: 0 });
}

fn remove(core: &mut Harness, path: &Path) -> Vec<Event> {
    core.send(Command::RemoveWorktree { worktree_path: path.to_string_lossy().into_owned() })
}

#[test]
fn removing_a_dirty_worktree_is_refused_and_names_the_reason() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/login");
    std::fs::write(worktree.join("README"), "edited\n").unwrap();
    std::fs::write(worktree.join("new.txt"), "new\n").unwrap();
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    let before = harness::worktree_snapshot(&worktree);

    let reason = refused(&remove(&mut core, &worktree));

    assert!(reason.contains("feat/login") && reason.contains("2 uncommitted files"), "{reason}");
    assert!(git_worktree_paths(&repo.path).contains(&worktree));
    assert_eq!(harness::worktree_snapshot(&worktree), before, "nothing touched");
}

#[test]
fn removing_a_clean_worktree_runs_git_worktree_remove_and_keeps_the_branch() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/login");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));

    let events = remove(&mut core, &worktree);

    assert_eq!(
        events,
        vec![Event::WorktreeRemoved {
            repository_id: repository.id,
            worktree_path: worktree.to_string_lossy().into_owned(),
        }]
    );
    assert!(!worktree.exists());
    assert!(!git_worktree_paths(&repo.path).contains(&worktree));
    harness::git(&repo.path, &["show-ref", "--verify", "refs/heads/feat/login"]);
}

#[test]
fn the_primary_worktree_is_never_removed() {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    core.add_repository(&repo.path);

    let reason = refused(&remove(&mut core, &repo.path));

    assert!(reason.contains("primary"), "{reason}");
    assert!(repo.path.join("README").is_file());
}

fn start_on_new(core: &mut Harness, repository_id: &str, branch: &str, mode: PermissionMode, prompt: &str) -> Vec<Event> {
    core.send(Command::StartSessionOnNewWorktree {
        repository_id: repository_id.into(),
        branch: branch.into(),
        agent: AgentKind::ClaudeCode,
        permission_mode: mode,
        model: Default::default(),
        prompt: prompt.into(),
    })
}

#[test]
fn create_new_from_base_makes_the_worktree_then_starts_the_session_on_it() {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));

    let events = start_on_new(&mut core, &repository.id, "feat/login", PermissionMode::AskFirst, "Add a login form.");

    let worktree = added_worktree(&events);
    started_session(&events);
    assert!(PathBuf::from(&worktree.path).starts_with(core.worktree_root()));
    let session = events
        .iter()
        .find_map(|e| match e {
            Event::SessionStarted { session } => Some(session.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(session.worktree_path, worktree.path);
    assert_eq!(session.branch.as_deref(), Some("feat/login"));
    assert_eq!(core.agents.launches()[0].cwd, PathBuf::from(&worktree.path));
}

#[test]
fn create_new_from_base_with_no_prompt_creates_nothing() {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));

    let reason = refused(&start_on_new(&mut core, &repository.id, "feat/login", PermissionMode::AskFirst, "  "));

    assert!(reason.contains("prompt"), "{reason}");
    assert_eq!(git_worktree_paths(&repo.path), vec![repo.path.clone()]);
    assert!(core.agents.launches().is_empty());
}

#[test]
fn create_new_from_base_with_an_unsupported_cli_creates_nothing() {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));
    core.probe.cli_prints(None);

    let events = start_on_new(&mut core, &repository.id, "feat/login", PermissionMode::AskFirst, "Add a login form.");

    assert!(
        events.iter().any(|e| matches!(e, Event::CommandRefused { command, .. } if command == "startSessionOnNewWorktree")),
        "{events:#?}"
    );
    assert!(!events.iter().any(|e| matches!(e, Event::WorktreeAdded { .. })), "{events:#?}");
    assert_eq!(git_worktree_paths(&repo.path), vec![repo.path.clone()]);
}

#[test]
fn a_branch_that_already_exists_is_refused_with_the_reason() {
    let repo = TempRepo::new("acme");
    repo.add_worktree("feat/login");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));

    let reason = refused(&create(&mut core, &repository.id, "feat/login"));

    assert!(reason.contains("feat/login") && reason.contains("already exists"), "{reason}");
}

#[test]
fn a_worktree_with_a_session_on_it_is_not_removed() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/login");
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    core.start_session(&worktree, "Add a login form.");

    let reason = refused(&remove(&mut core, &worktree));

    assert!(reason.contains("Running"), "{reason}");
    assert!(worktree.is_dir());
}
