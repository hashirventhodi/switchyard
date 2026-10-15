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
