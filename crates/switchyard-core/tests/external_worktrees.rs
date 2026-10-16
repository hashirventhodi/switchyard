//! Switchyard and git agree about which Worktrees exist (ADR-0008): a Worktree added,
//! removed or renamed in a shell is seen when the watcher reports the change under the
//! Repository's `.git`. The watcher is fake: `disk_changed` delivers a notification by hand.
mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, git, started_session, Harness, TempRepo};
use switchyard_schema::{Command, Event, Repository, SessionStatus, Worktree};

/// A repository with no linked worktrees, added and attached.
fn attached() -> (TempRepo, Repository, Harness) {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    core.send(Command::Attach);
    let repository = added_repository(&core.add_repository(&repo.path));
    (repo, repository, core)
}

/// The worktrees announced in `events`.
fn added(events: &[Event]) -> Vec<(String, Worktree)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorktreeAdded { repository_id, worktree } => Some((repository_id.clone(), worktree.clone())),
            _ => None,
        })
        .collect()
}

/// The worktree paths removed in `events`.
fn removed(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorktreeRemoved { repository_id, worktree_path } => {
                Some((repository_id.clone(), worktree_path.clone()))
            }
            _ => None,
        })
        .collect()
}

/// `git worktree add -b <branch>` in a shell, next to the repository; returns its path.
fn add_in_shell(repo: &TempRepo, branch: &str) -> PathBuf {
    repo.add_worktree(branch)
}

/// The linked worktree's own gitdir in the repository's `.git`.
fn gitdir_of(repo: &TempRepo, worktree: &Path) -> PathBuf {
    repo.path.join(".git/worktrees").join(worktree.file_name().unwrap())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn a_worktree_added_in_a_shell_is_announced_with_its_branch() {
    let (repo, repository, mut core) = attached();

    let worktree = add_in_shell(&repo, "feat/login");
    let events = core.disk_changed(&repo.path, &[&gitdir_of(&repo, &worktree).join("gitdir")]);

    assert_eq!(
        added(&events),
        vec![(
            repository.id,
            Worktree { path: path_string(&worktree), branch: Some("feat/login".into()), primary: false }
        )]
    );
}

#[test]
fn a_worktree_added_in_a_shell_is_watched_and_its_facts_pushed() {
    let (repo, _repository, mut core) = attached();

    let worktree = add_in_shell(&repo, "feat/login");
    let events = core.disk_changed(&repo.path, &[&repo.path.join(".git/worktrees")]);

    assert!(core.watcher.watching().contains(&worktree));
    assert!(events.iter().any(|e| matches!(e, Event::WorktreeFactsChanged { worktree_path, .. }
        if Path::new(worktree_path) == worktree)));
}

#[test]
fn a_change_that_adds_no_worktree_announces_none() {
    let (repo, _repository, mut core) = attached();

    let events = core.disk_changed(&repo.path, &[&repo.path.join(".git/refs/heads/main")]);

    assert_eq!(added(&events), vec![]);
}

#[test]
fn a_worktree_whose_branch_is_renamed_in_a_shell_is_announced_again_under_the_new_name() {
    let (repo, repository, mut core) = attached();
    let worktree = add_in_shell(&repo, "feat/login");
    core.disk_changed(&repo.path, &[&gitdir_of(&repo, &worktree).join("gitdir")]);

    git(&worktree, &["branch", "-m", "feat/sign-in"]);
    let events = core.disk_changed(&worktree, &[&gitdir_of(&repo, &worktree).join("HEAD")]);

    assert_eq!(
        added(&events),
        vec![(
            repository.id,
            Worktree { path: path_string(&worktree), branch: Some("feat/sign-in".into()), primary: false }
        )]
    );
    assert_eq!(removed(&events), vec![]);
}

#[test]
fn a_worktree_removed_in_a_shell_is_announced_gone_and_no_longer_watched() {
    let (repo, repository, mut core) = attached();
    let worktree = add_in_shell(&repo, "feat/login");
    core.disk_changed(&repo.path, &[&gitdir_of(&repo, &worktree).join("gitdir")]);

    git(&repo.path, &["worktree", "remove", worktree.to_str().unwrap()]);
    let events = core.disk_changed(&worktree, &[&gitdir_of(&repo, &worktree)]);

    assert_eq!(removed(&events), vec![(repository.id, path_string(&worktree))]);
    assert_eq!(core.watcher.watching(), vec![repo.path.clone()]);
}

#[test]
fn a_running_session_on_a_worktree_removed_in_a_shell_is_stopped_and_archived_with_its_transcript() {
    let (repo, _repository, mut core) = attached();
    let worktree = add_in_shell(&repo, "feat/login");
    core.disk_changed(&repo.path, &[&gitdir_of(&repo, &worktree).join("gitdir")]);
    let session = started_session(&core.start_session(&worktree, "Add a login form."));

    git(&repo.path, &["worktree", "remove", "--force", worktree.to_str().unwrap()]);
    let events = core.disk_changed(&repo.path, &[&gitdir_of(&repo, &worktree)]);

    let archived = events
        .iter()
        .find_map(|e| match e {
            Event::SessionArchived { archived } => Some(archived.clone()),
            _ => None,
        })
        .expect("the session is archived");
    assert_eq!(archived.session.id, session);
    assert_eq!(archived.session.status, SessionStatus::Idle, "stopped");
    assert!(archived.entries > 0, "its transcript is kept");
    assert!(core.processes.killed().len() == 1, "its agent is stopped");
    assert_eq!(removed(&events).len(), 1);

    let attach = core.send(Command::Attach);
    assert!(!attach.iter().any(|e| matches!(e, Event::SessionStarted { session } if session.id == session_id(&archived))));
}

fn session_id(archived: &switchyard_schema::ArchivedSession) -> String {
    archived.session.id.clone()
}

#[test]
fn a_worktree_removed_in_a_shell_closes_its_shells() {
    let (repo, _repository, mut core) = attached();
    let worktree = add_in_shell(&repo, "feat/login");
    core.disk_changed(&repo.path, &[&gitdir_of(&repo, &worktree).join("gitdir")]);
    core.use_shell("/bin/sh");
    core.dispatch(Command::OpenShell { worktree_path: path_string(&worktree), cols: 80, rows: 24 });
    core.wait_for(|seen| seen.iter().any(|e| matches!(e, Event::ShellOpened { .. })));

    git(&repo.path, &["worktree", "remove", worktree.to_str().unwrap()]);
    let events = core.disk_changed(&repo.path, &[&gitdir_of(&repo, &worktree)]);

    assert!(events.iter().any(|e| matches!(e, Event::ShellClosed { .. })), "{events:#?}");
}
