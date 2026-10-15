mod harness;

use harness::{added_repository, Harness, TempRepo};
use switchyard_schema::{Command, Event, Worktree};

#[test]
fn adding_a_repository_reports_its_base_branch_and_every_worktree() {
    let repo = TempRepo::new("acme");
    let feature = repo.add_worktree("feat/login");
    let mut core = Harness::launch();

    let events = core.add_repository(&repo.path);

    let repository = added_repository(&events);
    assert_eq!(repository.name, "acme");
    assert_eq!(repository.path, repo.path.to_str().unwrap());
    assert_eq!(repository.base_branch, "main");
    assert_eq!(
        events,
        vec![
            Event::RepositoryAdded { repository: repository.clone() },
            Event::WorktreeAdded {
                repository_id: repository.id.clone(),
                worktree: Worktree {
                    path: repo.path.to_str().unwrap().into(),
                    branch: Some("main".into()),
                    primary: true,
                },
            },
            Event::WorktreeAdded {
                repository_id: repository.id.clone(),
                worktree: Worktree {
                    path: feature.to_str().unwrap().into(),
                    branch: Some("feat/login".into()),
                    primary: false,
                },
            },
        ]
    );
}

#[test]
fn on_relaunch_repositories_are_replayed_with_worktrees_re_read_from_git() {
    let repo = TempRepo::new("acme");
    let doomed = repo.add_worktree("old/branch");
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));

    // While the app is closed, worktrees change behind its back.
    harness::git(&repo.path, &["worktree", "remove", doomed.to_str().unwrap()]);
    let fresh = repo.add_worktree("fix/typo");
    let mut core = core.relaunch();

    let events = core.send(Command::Attach);
    // Agent and gh detection (compat.rs, forge.rs) and settings (persistence.rs) are not part of this story.
    let events: Vec<_> = events
        .into_iter()
        .filter(|e| !matches!(e, Event::AgentsChecked { .. } | Event::SettingsChanged { .. } | Event::ForgeChecked { .. }))
        .collect();

    let worktree = |path: &std::path::Path, branch: &str, primary| Event::WorktreeAdded {
        repository_id: repository.id.clone(),
        worktree: Worktree {
            path: path.to_str().unwrap().into(),
            branch: Some(branch.into()),
            primary,
        },
    };
    assert_eq!(
        events,
        vec![
            Event::Attached,
            Event::RepositoryAdded { repository: repository.clone() },
            worktree(&repo.path, "main", true),
            worktree(&fresh, "fix/typo", false),
        ]
    );
}

#[test]
fn attaching_twice_replays_the_same_state() {
    let repo = TempRepo::new("acme");
    let mut core = Harness::launch();
    core.add_repository(&repo.path);

    let first = core.send(Command::Attach);
    let second = core.send(Command::Attach);

    assert_eq!(first.len(), 6, "attached, agents, forge, repository, worktree, settings: {first:#?}");
    assert_eq!(first, second);
}

#[test]
fn adding_a_repository_twice_is_refused() {
    let repo = TempRepo::new("acme");
    let feature = repo.add_worktree("feat/x");
    let mut core = Harness::launch();
    core.add_repository(&repo.path);

    // Picking a linked worktree of the same repository counts as the same repository.
    let events = core.add_repository(&feature);

    assert!(
        matches!(&events[..], [Event::CommandRefused { reason, .. }] if reason.contains("already added")),
        "{events:#?}"
    );
}

#[test]
fn a_folder_that_is_not_a_git_repository_is_refused_with_the_reason() {
    let folder = tempfile::TempDir::new().unwrap();
    let mut core = Harness::launch();

    let events = core.add_repository(folder.path());

    assert_eq!(events.len(), 1, "{events:#?}");
    match &events[0] {
        Event::CommandRefused { command, reason } => {
            assert_eq!(command, "addRepository");
            assert!(reason.contains("not a git repository"), "reason: {reason}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_folder_that_does_not_exist_is_refused() {
    let mut core = Harness::launch();

    let events = core.add_repository(std::path::Path::new("/definitely/not/here"));

    assert!(
        matches!(&events[..], [Event::CommandRefused { reason, .. }] if reason.contains("does not exist")),
        "{events:#?}"
    );
}
