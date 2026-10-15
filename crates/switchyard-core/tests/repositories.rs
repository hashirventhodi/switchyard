mod harness;

use harness::{added_repository, Harness, TempRepo};
use switchyard_schema::{Event, Worktree};

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
