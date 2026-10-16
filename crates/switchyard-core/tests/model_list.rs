//! The Model list through seam 1: what the New session picker offers comes from the CLI
//! itself (a `list_models` exchange with a running Session's agent), cached per agent and
//! CLI version with the time it was fetched, and read from that cache without ever waiting
//! on a CLI.
mod harness;

use std::path::PathBuf;

use adapter_protocol::{CliInfo, Command as AgentCommand, Event as AgentEvent, ListedModel as Offered, PermissionMode as ProtocolMode};
use harness::{added_repository, started_session, Harness, TempRepo, NOW_MS};
use switchyard_schema::{AgentKind, Command, Event, ListedModel, ModelList};

fn setup() -> (Harness, TempRepo, PathBuf) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    (core, repo, worktree)
}

fn list_requests(core: &Harness, agent: usize) -> usize {
    core.agents.all_commands(agent).iter().filter(|c| **c == AgentCommand::ListModels).count()
}

#[test]
fn a_session_start_asks_its_cli_which_models_it_offers() {
    let (mut core, _repo, worktree) = setup();

    started_session(&core.start_session(&worktree, "Fix it."));

    assert_eq!(list_requests(&core, 0), 1);
}

fn models_changed(events: &[Event], agent: AgentKind) -> Vec<ModelList> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ModelsChanged { agent: a, models } if *a == agent => Some(models.clone()),
            _ => None,
        })
        .collect()
}

fn read_models(core: &mut Harness, agent: AgentKind) -> ModelList {
    let changed = models_changed(&core.send(Command::ReadModels { agent }), agent);
    assert_eq!(changed.len(), 1, "one answer, from the cache: {changed:#?}");
    changed[0].clone()
}

#[test]
fn a_relaunched_agent_is_asked_too() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init("2.1.300"));
    core.agent_exits(0, Some(1), "");

    core.send(Command::SendPrompt { session_id: id, text: "Go on.".into() });

    assert_eq!(list_requests(&core, 1), 1);
    assert!(matches!(core.agents.all_commands(1)[0], AgentCommand::Resume { .. }), "resume stays the first command");
}

#[test]
fn before_any_session_new_session_reads_an_empty_list_at_once() {
    let (mut core, _repo, _worktree) = setup();

    assert_eq!(read_models(&mut core, AgentKind::ClaudeCode), ModelList::Empty);
    assert!(core.agents.launches().is_empty(), "no CLI is started to list Models");
}

fn init(version: &str) -> AgentEvent {
    AgentEvent::SessionInit {
        external_session_id: "cli-1".into(),
        permission_mode: ProtocolMode::AskFirst,
        cli_permission_mode: "default".into(),
        cli: CliInfo { name: "claude-code".into(), version: version.into() },
        model: Some("claude-opus-5-5".into()),
        mcp_servers: vec![],
    }
}

fn offered(ids: &[&str]) -> AgentEvent {
    AgentEvent::ModelsListed {
        models: ids.iter().map(|id| Offered { id: id.to_string(), display_name: id.to_uppercase(), resolved_model: None }).collect(),
    }
}

fn listed(ids: &[&str]) -> ModelList {
    ModelList::Listed {
        models: ids.iter().map(|id| ListedModel { id: id.to_string(), display_name: id.to_uppercase(), resolved_model: None }).collect(),
        fetched_at_ms: NOW_MS,
    }
}

#[test]
fn the_models_a_cli_lists_are_cached_with_when_they_were_fetched() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init("2.1.300"));

    let pushed = models_changed(&core.agent_says(0, offered(&["opus", "sonnet"])), AgentKind::ClaudeCode);

    assert_eq!(pushed, vec![listed(&["opus", "sonnet"])], "a dialog already open gets the list");
    assert_eq!(read_models(&mut core, AgentKind::ClaudeCode), listed(&["opus", "sonnet"]));
    let mut core = core.relaunch();
    assert_eq!(read_models(&mut core, AgentKind::ClaudeCode), listed(&["opus", "sonnet"]), "kept in the store");
}

#[test]
fn attaching_pushes_each_agents_cached_list_so_the_composer_has_it_too() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init("2.1.300"));
    core.agent_says(0, offered(&["opus", "sonnet"]));

    let attach = core.send(Command::Attach);

    assert_eq!(models_changed(&attach, AgentKind::ClaudeCode), vec![listed(&["opus", "sonnet"])]);
    assert_eq!(list_requests(&core, 0), 1, "from the cache: no CLI is asked");
}

#[test]
fn a_cli_that_cannot_list_models_is_remembered_as_such() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init("2.1.300"));

    core.agent_says(0, AgentEvent::CannotListModels { reason: Some("no model list here".into()) });

    assert_eq!(
        read_models(&mut core, AgentKind::ClaudeCode),
        ModelList::CannotList { reason: Some("no model list here".into()) }
    );
}

#[test]
fn a_failed_list_never_replaces_one_the_cli_gave() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init("2.1.300"));
    core.agent_says(0, offered(&["opus"]));

    core.agent_says(0, AgentEvent::CannotListModels { reason: Some("timed out".into()) });

    assert_eq!(read_models(&mut core, AgentKind::ClaudeCode), listed(&["opus"]));
}

#[test]
fn opening_new_session_asks_a_live_session_for_a_fresh_list_and_pushes_it_when_it_arrives() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init("2.1.300"));
    core.agent_says(0, offered(&["opus"]));

    assert_eq!(read_models(&mut core, AgentKind::ClaudeCode), listed(&["opus"]), "the cache, at once");
    assert_eq!(list_requests(&core, 0), 2, "and a refresh in the background");

    let pushed = models_changed(&core.agent_says(0, offered(&["opus", "sonnet"])), AgentKind::ClaudeCode);
    assert_eq!(pushed, vec![listed(&["opus", "sonnet"])]);
}

#[test]
fn opening_new_session_for_another_agent_asks_no_claude_code_session() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init("2.1.300"));

    assert_eq!(read_models(&mut core, AgentKind::Codex), ModelList::Empty);
    assert_eq!(list_requests(&core, 0), 1, "only the start's own request");
}

#[test]
fn a_cli_upgrade_shows_the_new_versions_list_not_the_old_ones() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init("2.1.300"));
    core.agent_says(0, offered(&["opus"]));

    core.probe.cli_prints(Some("2.1.301 (Claude Code)"));
    core.send(Command::CheckAgents);

    assert_eq!(read_models(&mut core, AgentKind::ClaudeCode), ModelList::Empty);
    assert_eq!(list_requests(&core, 0), 1, "a Session still on the old version is not asked");
    let pushed = models_changed(&core.agent_says(0, offered(&["opus", "sonnet"])), AgentKind::ClaudeCode);
    assert_eq!(pushed, vec![], "the old version's list is never shown for the new one");
}
