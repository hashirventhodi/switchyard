//! The core watches every Worktree on disk and pushes its facts when something changes
//! (ADR-0008). The watcher is fake here: `disk_changed` delivers a notification by hand and
//! returns the events it caused.
mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, git, Harness, TempRepo};
use switchyard_core::watcher::{FsWatcher, Watch, WatchReport, Watcher};
use switchyard_schema::{ChangeKind, Command, Event, WorktreeFacts};

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

    assert_eq!(facts_of(&events, &worktree), WorktreeFacts { ahead: 0, behind: 0, dirty: 1, conflicts: vec![], changed_files: 1, commits: 0 });
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

    assert_eq!(facts_of(&events, &worktree), WorktreeFacts { ahead: 1, behind: 0, dirty: 0, conflicts: vec![], changed_files: 0, commits: 1 });
}

#[test]
fn base_moving_in_a_shell_pushes_behind_for_the_worktree() {
    let (repo, worktree, mut core) = attached();

    // A pull or fetch into main from the Primary worktree.
    std::fs::write(repo.path.join("CHANGELOG"), "base moved\n").unwrap();
    git(&repo.path, &["add", "CHANGELOG"]);
    git(&repo.path, &["commit", "-q", "-m", "base moved"]);
    let events = core.disk_changed(&worktree, &[&repo.path.join(".git/refs/heads/main")]);

    assert_eq!(facts_of(&events, &worktree), WorktreeFacts { ahead: 0, behind: 1, dirty: 0, conflicts: vec![], changed_files: 0, commits: 0 });
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

    let clean = WorktreeFacts { ahead: 0, behind: 0, dirty: 0, conflicts: vec![], changed_files: 0, commits: 0 };
    assert_eq!(facts_of(&events, &repo.path), clean);
    assert_eq!(facts_of(&events, &login), clean);
    assert_eq!(facts_of(&events, &cache), WorktreeFacts { dirty: 1, changed_files: 1, ..clean.clone() });
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
    // Beside the configuration it is watched for (the local tasks file).
    let primary = core.watcher.roots(&repo.path);
    assert_eq!(primary[0], repo.path);
    assert!(!primary.iter().any(|root| root.ends_with(".git")), "its .git is inside it: {primary:?}");
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

/// The kinds of the `WorktreeChanged` pushed for `path` in `events` (exactly one).
fn changed_kinds(events: &[Event], path: &Path) -> Vec<ChangeKind> {
    let found: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorktreeChanged { worktree_path, kinds } if Path::new(worktree_path) == path => Some(kinds.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(found.len(), 1, "expected one change signal for {path:?} in {events:#?}");
    found.into_iter().next().unwrap()
}

#[test]
fn a_file_edit_signals_files_beside_facts_counting_what_changes_would_list() {
    let (_repo, worktree, mut core) = attached();

    std::fs::write(worktree.join("README"), "edited in another editor\n").unwrap();
    std::fs::write(worktree.join("notes.txt"), "scratch\n").unwrap();
    // Staged only: Changes lists the working tree against the index, so not this one.
    std::fs::write(worktree.join("staged.txt"), "staged\n").unwrap();
    git(&worktree, &["add", "staged.txt"]);
    let events = core.disk_changed(&worktree, &[&worktree.join("README"), &worktree.join("notes.txt")]);

    assert_eq!(changed_kinds(&events, &worktree), vec![ChangeKind::Files]);
    let facts = facts_of(&events, &worktree);
    assert_eq!((facts.dirty, facts.changed_files), (3, 2));
    let reads = events.iter().filter(|e| matches!(e, Event::ChangesRead { .. })).count();
    assert_eq!(reads, 0, "no diff is computed for a worktree nobody asked to read");
}

#[test]
fn a_file_renamed_on_disk_counts_once_as_changes_lists_it() {
    let (_repo, worktree, mut core) = attached();

    std::fs::rename(worktree.join("README"), worktree.join("README.md")).unwrap();
    let events = core.disk_changed(&worktree, &[&worktree.join("README"), &worktree.join("README.md")]);

    let read = core.send(Command::ReadChanges { worktree_path: worktree.to_string_lossy().into_owned() });
    let listed = read.iter().find_map(|e| match e {
        Event::ChangesRead { files, .. } => Some(files.len()),
        _ => None,
    });
    assert_eq!(listed, Some(1));
    assert_eq!(facts_of(&events, &worktree).changed_files, 1);
}

#[test]
fn a_commit_signals_git_with_the_commits_history_would_list() {
    let (repo, worktree, mut core) = attached();

    for name in ["login.rs", "logout.rs"] {
        std::fs::write(worktree.join(name), "fn f() {}\n").unwrap();
        git(&worktree, &["add", name]);
        git(&worktree, &["commit", "-q", "-m", name]);
    }
    let events = core.disk_changed(&worktree, &[&repo.path.join(".git/refs/heads/feat/login")]);

    assert_eq!(changed_kinds(&events, &worktree), vec![ChangeKind::Git]);
    assert_eq!(facts_of(&events, &worktree).commits, 2);
}

#[test]
fn an_amend_that_leaves_ahead_unchanged_still_signals_git() {
    let (repo, worktree, mut core) = attached();
    std::fs::write(worktree.join("login.rs"), "fn login() {}\n").unwrap();
    git(&worktree, &["add", "login.rs"]);
    git(&worktree, &["commit", "-q", "-m", "login"]);
    core.disk_changed(&worktree, &[&repo.path.join(".git/refs/heads/feat/login")]);

    git(&worktree, &["commit", "-q", "--amend", "-m", "login, reworded"]);
    let events = core.disk_changed(&worktree, &[&gitdir_of(&repo, &worktree).join("logs/HEAD")]);

    assert_eq!(changed_kinds(&events, &worktree), vec![ChangeKind::Git]);
    assert_eq!(facts_of(&events, &worktree).commits, 1);
}

#[test]
fn a_file_edit_and_a_commit_together_signal_both_kinds() {
    let (repo, worktree, mut core) = attached();

    std::fs::write(worktree.join("README"), "edited\n").unwrap();
    let events = core.disk_changed(&worktree, &[&worktree.join("README"), &repo.path.join(".git/refs/heads/feat/login")]);

    assert_eq!(changed_kinds(&events, &worktree), vec![ChangeKind::Files, ChangeKind::Git]);
}

/// Paths the real `watcher` reports for `root` until `done` holds for them (or 10 s pass).
fn reported_until(rx: &std::sync::mpsc::Receiver<WatchReport>, done: impl Fn(&[PathBuf]) -> bool) -> Vec<PathBuf> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut seen = Vec::new();
    while !done(&seen) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(WatchReport::Changed(paths)) => seen.extend(paths),
            Ok(WatchReport::Failed(reason)) => panic!("the watch failed: {reason}"),
            Err(_) => panic!("timed out; reported so far: {seen:#?}"),
        }
    }
    seen
}

fn ends_with(paths: &[PathBuf], tail: &str) -> bool {
    paths.iter().any(|p| p.ends_with(tail))
}

#[test]
fn the_real_watcher_skipping_ignored_folders_watches_the_rest_and_folders_made_later() {
    let repo = TempRepo::new("acme");
    std::fs::write(repo.path.join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(repo.path.join("target/debug")).unwrap();
    std::fs::create_dir_all(repo.path.join("src")).unwrap();
    let watcher = FsWatcher::skipping_ignored_over(std::time::Duration::from_millis(100));
    let (tx, rx) = std::sync::mpsc::channel();
    let watch = Watch { worktree: repo.path.clone(), roots: vec![repo.path.clone()] };
    watcher.watch(watch, std::sync::Arc::new(move |report| drop(tx.send(report)))).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));

    std::fs::write(repo.path.join("target/debug/app"), "binary").unwrap();
    std::fs::write(repo.path.join("src/lib.rs"), "fn f() {}\n").unwrap();
    let seen = reported_until(&rx, |seen| ends_with(seen, "src/lib.rs"));
    assert!(!seen.iter().any(|p| p.components().any(|c| c.as_os_str() == "target")), "{seen:#?}");

    std::fs::create_dir_all(repo.path.join("src/deep")).unwrap();
    reported_until(&rx, |seen| ends_with(seen, "src/deep"));
    std::fs::write(repo.path.join("src/deep/mod.rs"), "fn g() {}\n").unwrap();
    reported_until(&rx, |seen| ends_with(seen, "src/deep/mod.rs"));
}

#[test]
fn another_worktrees_index_and_head_changing_pushes_nothing() {
    let (repo, worktree, mut core) = attached();
    let other = repo.add_worktree("spike/cache");
    core.disk_changed(&worktree, &[&gitdir_of(&repo, &other)]);

    let theirs = gitdir_of(&repo, &other);
    let events = core.disk_changed(
        &worktree,
        &[&theirs.join("index"), &theirs.join("HEAD"), &theirs.join("logs/HEAD"), &repo.path.join(".git/index"), &repo.path.join(".git/HEAD")],
    );

    assert_eq!(events, vec![]);
}

#[test]
fn a_worktree_added_beside_it_is_seen_from_its_gitdir_file() {
    let (repo, worktree, mut core) = attached();
    let other = repo.add_worktree("spike/cache");

    let events = core.disk_changed(&worktree, &[&gitdir_of(&repo, &other).join("gitdir")]);

    assert!(events.iter().any(|e| matches!(e, Event::WorktreeAdded { worktree, .. } if Path::new(&worktree.path) == other)), "{events:#?}");
}

#[test]
fn its_own_index_changing_signals_git() {
    let (repo, worktree, mut core) = attached();

    let events = core.disk_changed(&worktree, &[&gitdir_of(&repo, &worktree).join("index")]);

    assert_eq!(changed_kinds(&events, &worktree), vec![ChangeKind::Git]);
}

#[test]
fn the_primary_worktrees_own_index_changing_signals_git() {
    let (repo, _worktree, mut core) = attached();

    let events = core.disk_changed(&repo.path, &[&repo.path.join(".git/index")]);

    assert_eq!(changed_kinds(&events, &repo.path), vec![ChangeKind::Git]);
}
