//! A Session's Model through seam 1: chosen at start (a Model id, or Default, which passes
//! nothing to the CLI), never hidden once it runs (the Agent log names what Default
//! resolved to), and usage on an unresolved Default held unpriced until a concrete Model
//! name resolves it.
mod harness;

use std::path::PathBuf;

use harness::{added_repository, started_session, Harness, TempRepo};
use switchyard_schema::{ModelChoice, PermissionMode};

fn setup() -> (Harness, TempRepo, PathBuf) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    (core, repo, worktree)
}

fn model(id: &str) -> ModelChoice {
    ModelChoice::Model { id: id.into() }
}

#[test]
fn a_session_started_on_a_chosen_model_runs_its_cli_on_that_model() {
    let (mut core, _repo, worktree) = setup();

    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, model("claude-sonnet-5-5"), "Fix it."));

    assert_eq!(core.agents.launches()[0].model.as_deref(), Some("claude-sonnet-5-5"));
}

#[test]
fn a_session_started_on_default_passes_no_model_to_its_cli() {
    let (mut core, _repo, worktree) = setup();

    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, ModelChoice::Default, "Fix it."));

    assert_eq!(core.agents.launches()[0].model, None);
}
