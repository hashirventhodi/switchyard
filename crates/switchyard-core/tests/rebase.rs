//! Behind, rebase and conflict detection through seam 1, against real temporary git
//! repositories: ahead/behind and conflicting files read from git, a dry-run merge that
//! never touches the working tree or the index, rebase that completes or aborts cleanly,
//! and the branch's history above its base.
mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, git, worktree_snapshot, Harness, TempRepo};
use switchyard_schema::{Command, Event, WorktreeFacts};

/// A repository with an identity for the commits a rebase rewrites (CI has no global one),
/// added to a fresh core, plus a worktree on `feat/login` made through the core.
fn setup() -> (TempRepo, Harness, PathBuf) {
    let repo = TempRepo::new("acme");
    git(&repo.path, &["config", "user.name", "Test"]);
    git(&repo.path, &["config", "user.email", "test@example.com"]);
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));
    let events = core.send(Command::CreateWorktree { repository_id: repository.id, branch: "feat/login".into() });
    let path = events
        .iter()
        .find_map(|e| match e {
            Event::WorktreeAdded { worktree, .. } => Some(PathBuf::from(&worktree.path)),
            _ => None,
        })
        .expect("worktree created");
    (repo, core, path)
}

fn commit(dir: &Path, file: &str, text: &str, message: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn facts_of(events: &[Event], path: &Path) -> WorktreeFacts {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::WorktreeFactsChanged { worktree_path, facts } if Path::new(worktree_path) == path => Some(facts.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no facts for {path:?} in {events:#?}"))
}

fn refresh(core: &mut Harness) -> Vec<Event> {
    core.send(Command::RefreshWorktrees)
}

/// The index file's bytes: changes if anything is staged, unstaged or the index rewritten.
fn index_bytes(worktree: &Path) -> Vec<u8> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", "index"])
        .current_dir(worktree)
        .output()
        .unwrap();
    let index = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let index = if index.is_absolute() { index } else { worktree.join(index) };
    std::fs::read(index).unwrap()
}

#[test]
fn a_dry_run_merge_names_the_files_that_would_conflict_without_touching_the_tree_or_index() {
    let (repo, mut core, path) = setup();
    commit(&path, "README", "hello from the branch\n", "branch edit");
    commit(&repo.path, "README", "hello from main\n", "main edit");
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");
    let tree_before = worktree_snapshot(&path);
    let index_before = index_bytes(&path);

    let facts = facts_of(&refresh(&mut core), &path);

    assert_eq!(facts, WorktreeFacts { ahead: 1, behind: 2, dirty: 0, conflicts: vec!["README".into()] });
    assert_eq!(worktree_snapshot(&path), tree_before);
    assert_eq!(index_bytes(&path), index_before);
}

#[test]
fn a_branch_behind_with_disjoint_changes_has_no_conflicts() {
    let (repo, mut core, path) = setup();
    commit(&path, "login.rs", "fn login() {}\n", "add login");
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");

    let facts = facts_of(&refresh(&mut core), &path);

    assert_eq!(facts, WorktreeFacts { ahead: 1, behind: 1, dirty: 0, conflicts: vec![] });
}
