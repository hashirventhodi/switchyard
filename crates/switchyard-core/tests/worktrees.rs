//! Worktree lifecycle through seam 1: create into the Switchyard worktree root, adopt
//! hand-made worktrees in place, remove only clean ones, keep working sessions off primary.
mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, Harness, TempRepo};
use switchyard_schema::{Command, Event, Worktree, WorktreeFacts};

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
    assert_eq!(facts_of(&events, &path), WorktreeFacts { ahead: 0, behind: 0, dirty: 0 });

    std::fs::write(path.join("login.rs"), "fn login() {}\n").unwrap();
    harness::git(&path, &["add", "login.rs"]);
    harness::git(&path, &["commit", "-q", "-m", "login"]);
    std::fs::write(path.join("README"), "changed\n").unwrap();
    std::fs::write(path.join("notes.txt"), "scratch\n").unwrap();
    std::fs::write(repo.path.join("CHANGELOG"), "base moved\n").unwrap();
    harness::git(&repo.path, &["add", "CHANGELOG"]);
    harness::git(&repo.path, &["commit", "-q", "-m", "base moved"]);

    let events = core.send(Command::RefreshWorktrees);

    assert_eq!(facts_of(&events, &path), WorktreeFacts { ahead: 1, behind: 1, dirty: 2 });
    assert_eq!(facts_of(&events, &repo.path), WorktreeFacts { ahead: 0, behind: 0, dirty: 0 });
}
