//! The core watches every Worktree on disk and pushes its facts when something changes
//! (ADR-0008). The watcher is fake here: `disk_changed` delivers a notification by hand and
//! returns the events it caused.
mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, git, Harness, TempRepo};
use switchyard_core::watcher::FsWatcher;
use switchyard_schema::{Command, Event, WorktreeFacts};

/// The facts pushed for `path` in `events` (exactly one).
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

/// A repository with one linked worktree on `feat/login`, added and attached.
fn attached() -> (TempRepo, PathBuf, Harness) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/login");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    core.send(Command::Attach);
    (repo, worktree, core)
}

#[test]
fn an_edit_on_disk_pushes_the_worktrees_facts() {
    let (_repo, worktree, mut core) = attached();

    std::fs::write(worktree.join("README"), "edited in another editor\n").unwrap();
    let events = core.disk_changed(&worktree, &[&worktree.join("README")]);

    assert_eq!(facts_of(&events, &worktree), WorktreeFacts { ahead: 0, behind: 0, dirty: 1, conflicts: vec![] });
}

/// The linked worktree's own gitdir in the repository's `.git` (its HEAD, index, logs).
fn gitdir_of(repo: &TempRepo, worktree: &Path) -> PathBuf {
    repo.path.join(".git/worktrees").join(worktree.file_name().unwrap())
}

#[test]
fn a_commit_in_a_shell_pushes_the_worktrees_facts() {
    let (repo, worktree, mut core) = attached();

    std::fs::write(worktree.join("login.rs"), "fn login() {}\n").unwrap();
    git(&worktree, &["add", "login.rs"]);
    git(&worktree, &["commit", "-q", "-m", "login"]);
    let events = core.disk_changed(
        &worktree,
        &[&repo.path.join(".git/refs/heads/feat/login"), &gitdir_of(&repo, &worktree).join("logs/HEAD")],
    );

    assert_eq!(facts_of(&events, &worktree), WorktreeFacts { ahead: 1, behind: 0, dirty: 0, conflicts: vec![] });
}

#[test]
fn base_moving_in_a_shell_pushes_behind_for_the_worktree() {
    let (repo, worktree, mut core) = attached();

    // A pull or fetch into main from the primary checkout.
    std::fs::write(repo.path.join("CHANGELOG"), "base moved\n").unwrap();
    git(&repo.path, &["add", "CHANGELOG"]);
    git(&repo.path, &["commit", "-q", "-m", "base moved"]);
    let events = core.disk_changed(&worktree, &[&repo.path.join(".git/refs/heads/main")]);

    assert_eq!(facts_of(&events, &worktree), WorktreeFacts { ahead: 0, behind: 1, dirty: 0, conflicts: vec![] });
}

#[test]
fn a_change_under_an_ignored_directory_pushes_nothing() {
    let (_repo, worktree, mut core) = attached();
    std::fs::write(worktree.join(".gitignore"), "target/\n").unwrap();
    git(&worktree, &["add", ".gitignore"]);
    git(&worktree, &["commit", "-q", "-m", "ignore build output"]);

    std::fs::create_dir_all(worktree.join("target/debug")).unwrap();
    std::fs::write(worktree.join("target/debug/app"), "binary").unwrap();
    let events = core.disk_changed(&worktree, &[&worktree.join("target/debug/app"), &worktree.join("target/debug")]);

    assert_eq!(events, vec![]);
}

#[test]
fn attach_pushes_facts_for_every_worktree() {
    let repo = TempRepo::new("acme");
    let login = repo.add_worktree("feat/login");
    let cache = repo.add_worktree("spike/cache");
    std::fs::write(cache.join("notes.txt"), "scratch\n").unwrap();
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    let mut core = core.relaunch();

    let events = core.send(Command::Attach);

    let clean = WorktreeFacts { ahead: 0, behind: 0, dirty: 0, conflicts: vec![] };
    assert_eq!(facts_of(&events, &repo.path), clean);
    assert_eq!(facts_of(&events, &login), clean);
    assert_eq!(facts_of(&events, &cache), WorktreeFacts { dirty: 1, ..clean.clone() });
}

#[test]
fn adding_a_repository_pushes_facts_for_its_worktrees() {
    let repo = TempRepo::new("acme");
    let login = repo.add_worktree("feat/login");
    std::fs::write(login.join("README"), "edited\n").unwrap();
    let mut core = Harness::launch();
    core.send(Command::Attach);

    let events = core.add_repository(&repo.path);

    assert_eq!(facts_of(&events, &login).dirty, 1);
    assert_eq!(facts_of(&events, &repo.path).dirty, 0);
}

#[test]
fn every_known_worktree_is_watched_from_attach_with_its_repositorys_git_folder() {
    let (repo, worktree, core) = attached();

    let mut expected = vec![repo.path.clone(), worktree.clone()];
    expected.sort();
    assert_eq!(core.watcher.watching(), expected);
    assert_eq!(core.watcher.roots(&worktree), vec![worktree.clone(), repo.path.join(".git")]);
    assert_eq!(core.watcher.roots(&repo.path), vec![repo.path.clone()], "its .git is inside it");
}

#[test]
fn a_created_worktree_is_watched_and_a_removed_one_no_longer() {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    core.send(Command::Attach);
    let repository = added_repository(&core.add_repository(&repo.path));

    let events = core.send(Command::CreateWorktree { repository_id: repository.id, branch: "feat/login".into() });
    let created = events
        .iter()
        .find_map(|e| match e {
            Event::WorktreeAdded { worktree, .. } => Some(PathBuf::from(&worktree.path)),
            _ => None,
        })
        .expect("a worktree");
    assert!(core.watcher.watching().contains(&created));

    core.send(Command::RemoveWorktree { worktree_path: created.to_string_lossy().into_owned() });
    assert_eq!(core.watcher.watching(), vec![repo.path.clone()]);
    assert_eq!(core.disk_changed(&created, &[&created.join("README")]), vec![]);
}

#[test]
fn the_real_watcher_pushes_facts_after_an_edit_on_disk() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/login");
    let mut core = Harness::launch_watching(std::sync::Arc::new(FsWatcher::new()));
    core.add_repository(&repo.path);
    core.send(Command::Attach);

    std::fs::write(worktree.join("README"), "edited in another editor\n").unwrap();

    core.wait_for(|events| {
        events.iter().any(|e| {
            matches!(e, Event::WorktreeFactsChanged { worktree_path, facts }
                if Path::new(worktree_path) == worktree && facts.dirty == 1)
        })
    });

    // Reading the facts writes nothing the watcher sees: no change feeds itself.
    std::thread::sleep(std::time::Duration::from_secs(2));
    core.drain();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let later = core.drain();
    assert!(!later.iter().any(|e| matches!(e, Event::WorktreeFactsChanged { .. })), "{later:#?}");
}

#[test]
fn a_change_to_git_internals_other_than_its_records_pushes_nothing() {
    let (repo, worktree, mut core) = attached();

    let events = core.disk_changed(
        &worktree,
        &[&repo.path.join(".git/objects/ab/cdef"), &repo.path.join(".git/FETCH_HEAD"), &repo.path.join(".git/index.lock")],
    );

    assert_eq!(events, vec![]);
}
