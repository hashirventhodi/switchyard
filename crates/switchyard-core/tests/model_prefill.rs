//! New session's Model pre-fill through seam 1: the last Model *choice* made for an agent in
//! a Repository, kept in Switchyard's store (never in the Repository), with a remembered
//! Model the cached list no longer offers falling back to Default plus a note.
mod harness;

use std::path::{Path, PathBuf};

use adapter_protocol::{CliInfo, Event as AgentEvent, ListedModel as Offered, PermissionMode as ProtocolMode};
use harness::{added_repository, started_session, worktree_snapshot, Harness, TempRepo};
use switchyard_schema::{AgentKind, Command, Event, ModelChoice, PermissionMode};

fn setup() -> (Harness, TempRepo, String, PathBuf) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    let id = added_repository(&core.add_repository(&repo.path)).id;
    (core, repo, id, worktree)
}

fn opus() -> ModelChoice {
    ModelChoice::Model { id: "opus".into() }
}

/// What New session pre-fills for `agent` in `repository_id`: the choice and the retired note.
fn prefill(core: &mut Harness, agent: AgentKind, repository_id: &str) -> (ModelChoice, Option<String>) {
    let events = core.send(Command::ReadModelPrefill { agent, repository_id: repository_id.into() });
    let answers: Vec<_> = events
        .into_iter()
        .filter_map(|e| match e {
            Event::ModelPrefill { agent: a, repository_id: r, choice, retired } if a == agent && r == repository_id => {
                Some((choice, retired))
            }
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1, "one answer");
    answers[0].clone()
}

#[test]
fn before_any_choice_new_session_pre_fills_default() {
    let (mut core, _repo, id, _worktree) = setup();

    assert_eq!(prefill(&mut core, AgentKind::ClaudeCode, &id), (ModelChoice::Default, None));
}

#[test]
fn the_last_choice_for_an_agent_in_a_repository_is_pre_filled_and_kept_in_the_store() {
    let (mut core, _repo, id, worktree) = setup();

    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, opus(), "Fix it."));

    assert_eq!(prefill(&mut core, AgentKind::ClaudeCode, &id), (opus(), None));
    let mut core = core.relaunch();
    assert_eq!(prefill(&mut core, AgentKind::ClaudeCode, &id), (opus(), None), "kept across restarts");
}

fn start(core: &mut Harness, worktree: &Path, agent: AgentKind, model: ModelChoice) -> String {
    started_session(&core.send(Command::StartSession {
        worktree_path: worktree.to_string_lossy().into_owned(),
        agent,
        permission_mode: PermissionMode::AskFirst,
        model,
        prompt: "Fix it.".into(),
    }))
}

fn init(model: &str) -> AgentEvent {
    AgentEvent::SessionInit {
        external_session_id: "cli-1".into(),
        permission_mode: ProtocolMode::AskFirst,
        cli_permission_mode: "default".into(),
        cli: CliInfo { name: "claude-code".into(), version: "2.1.300".into() },
        model: Some(model.into()),
        mcp_servers: vec![],
    }
}

fn offered(ids: &[&str]) -> AgentEvent {
    AgentEvent::ModelsListed {
        models: ids.iter().map(|id| Offered { id: id.to_string(), display_name: id.to_uppercase() }).collect(),
    }
}

#[test]
fn default_is_remembered_as_default_not_as_the_model_it_resolved_to() {
    let (mut core, repo, id, worktree) = setup();
    start(&mut core, &worktree, AgentKind::ClaudeCode, opus());
    let other = repo.add_worktree("fix/other");

    start(&mut core, &other, AgentKind::ClaudeCode, ModelChoice::Default);
    core.agent_says(1, init("claude-opus-5-5"));

    assert_eq!(prefill(&mut core, AgentKind::ClaudeCode, &id), (ModelChoice::Default, None));
}

#[test]
fn the_choice_is_remembered_per_agent_and_per_repository() {
    let (mut core, _repo, id, worktree) = setup();
    let other_repo = TempRepo::new("billing");
    let other = added_repository(&core.add_repository(&other_repo.path)).id;

    start(&mut core, &worktree, AgentKind::ClaudeCode, opus());

    assert_eq!(prefill(&mut core, AgentKind::ClaudeCode, &other), (ModelChoice::Default, None), "another Repository");
    assert_eq!(prefill(&mut core, AgentKind::Codex, &id), (ModelChoice::Default, None), "another agent");
}

#[test]
fn a_session_that_never_starts_leaves_the_last_choice_alone() {
    let (mut core, repo, id, worktree) = setup();
    start(&mut core, &worktree, AgentKind::ClaudeCode, opus());
    let other = repo.add_worktree("fix/other");

    core.agents.refuse_next_launch("no such file");
    core.send(Command::StartSession {
        worktree_path: other.to_string_lossy().into_owned(),
        agent: AgentKind::ClaudeCode,
        permission_mode: PermissionMode::AskFirst,
        model: ModelChoice::Model { id: "sonnet".into() },
        prompt: "Fix it.".into(),
    });

    assert_eq!(prefill(&mut core, AgentKind::ClaudeCode, &id), (opus(), None));
}

#[test]
fn a_remembered_model_the_cached_list_no_longer_offers_pre_fills_default_with_a_note() {
    let (mut core, _repo, id, worktree) = setup();
    start(&mut core, &worktree, AgentKind::ClaudeCode, opus());
    core.agent_says(0, init("claude-opus-5-5"));

    core.agent_says(0, offered(&["sonnet", "haiku"]));

    assert_eq!(prefill(&mut core, AgentKind::ClaudeCode, &id), (ModelChoice::Default, Some("opus".into())));
}

#[test]
fn a_remembered_model_the_cached_list_offers_is_pre_filled() {
    let (mut core, _repo, id, worktree) = setup();
    start(&mut core, &worktree, AgentKind::ClaudeCode, opus());
    core.agent_says(0, init("claude-opus-5-5"));

    core.agent_says(0, offered(&["opus", "sonnet"]));

    assert_eq!(prefill(&mut core, AgentKind::ClaudeCode, &id), (opus(), None));
}

#[test]
fn with_an_empty_cache_the_remembered_id_is_offered_unchanged() {
    let (mut core, _repo, id, worktree) = setup();
    start(&mut core, &worktree, AgentKind::ClaudeCode, ModelChoice::Model { id: "claude-opus-4-1".into() });
    core.agent_says(0, init("claude-opus-4-1"));
    core.agent_says(0, offered(&["opus"]));

    core.probe.cli_prints(Some("2.1.301 (Claude Code)"));
    core.send(Command::CheckAgents);

    assert_eq!(
        prefill(&mut core, AgentKind::ClaudeCode, &id),
        (ModelChoice::Model { id: "claude-opus-4-1".into() }, None),
        "the upgraded CLI has no list yet, so nothing says the id is gone"
    );
}

#[test]
fn nothing_is_written_to_the_repository() {
    let (mut core, repo, id, worktree) = setup();
    let (before, worktree_before) = (worktree_snapshot(&repo.path), worktree_snapshot(&worktree));

    start(&mut core, &worktree, AgentKind::ClaudeCode, opus());
    prefill(&mut core, AgentKind::ClaudeCode, &id);

    assert_eq!(worktree_snapshot(&repo.path), before);
    assert_eq!(worktree_snapshot(&worktree), worktree_before);
}
