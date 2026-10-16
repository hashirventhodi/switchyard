//! A Session's Model through seam 1: chosen at start (a Model id, or Default, which passes
//! nothing to the CLI), never hidden once it runs (the Agent log names what Default
//! resolved to), and usage on an unresolved Default held unpriced until a concrete Model
//! name resolves it.
mod harness;

use std::path::PathBuf;

use adapter_protocol::{CliInfo, Event as AgentEvent, PermissionMode as ProtocolMode};
use harness::{added_repository, recorded_events, started_session, Harness, TempRepo};
use switchyard_schema::{AgentKind, Command, Entry, Event, ModelChoice, PermissionMode, SessionFacts, Spend};

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

fn init(model: Option<&str>) -> AgentEvent {
    AgentEvent::SessionInit {
        external_session_id: "cli-1".into(),
        permission_mode: ProtocolMode::AskFirst,
        cli_permission_mode: "default".into(),
        cli: CliInfo { name: "claude-code".into(), version: "2.1.300".into() },
        model: model.map(str::to_string),
        mcp_servers: vec![],
    }
}

/// The text of every system line (Agent log `#` line) in `events`.
fn system_lines(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { entry: Entry::System { text }, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The Session's facts as last reported in `events`.
fn last_facts(events: &[Event]) -> SessionFacts {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionFactsChanged { facts, .. } => Some(facts.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no facts in {events:#?}"))
}

#[test]
fn the_agent_log_names_what_default_resolved_to_at_start() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));

    let events = core.agent_replays(0, &recorded_events("claude-code/first-session"));

    assert_eq!(system_lines(&events), vec!["Model: Default → claude-opus-5-5".to_string()]);
}

#[test]
fn the_agent_log_names_the_chosen_model_at_start() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, model("claude-sonnet-5-5"), "Fix it."));

    let events = core.agent_says(0, init(Some("claude-sonnet-5-5")));

    assert_eq!(system_lines(&events), vec!["Model: claude-sonnet-5-5".to_string()]);
    assert_eq!(last_facts(&events).model.as_deref(), Some("claude-sonnet-5-5"));
}

#[test]
fn a_default_the_cli_does_not_report_is_logged_as_not_reported_and_never_guessed() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix it."));

    let events = core.agent_says(0, init(None));

    assert_eq!(system_lines(&events), vec!["Model: Default (not reported)".to_string()]);
    assert_eq!(last_facts(&events).model, None);
}

#[test]
fn a_chosen_model_the_cli_does_not_report_is_the_one_it_was_started_on() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, model("claude-sonnet-5-5"), "Fix it."));

    let events = core.agent_says(0, init(None));

    assert_eq!(system_lines(&events), vec!["Model: claude-sonnet-5-5".to_string()]);
    assert_eq!(last_facts(&events).model.as_deref(), Some("claude-sonnet-5-5"));
}

#[test]
fn an_explore_session_starts_on_its_chosen_model_too() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session_on(&worktree, PermissionMode::Explore, model("claude-haiku-4-5"), "Look around."));

    let events = core.agent_says(0, init(Some("claude-haiku-4-5")));

    assert_eq!(core.agents.launches()[0].model.as_deref(), Some("claude-haiku-4-5"));
    assert_eq!(system_lines(&events), vec!["Model: claude-haiku-4-5".to_string()]);
}

#[test]
fn the_model_line_is_written_once_and_a_resumed_cli_does_not_repeat_it() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init(Some("claude-opus-5-5")));
    core.agent_says(0, AgentEvent::TurnComplete);
    core.agent_exits(0, Some(0), "");

    core.send(Command::SendPrompt { session_id: id, text: "Go on.".into() });
    assert_eq!(core.agents.launches().len(), 2, "the gone agent is relaunched by its id");
    let events = core.agent_says(1, init(Some("claude-opus-5-5")));

    assert_eq!(system_lines(&events), Vec::<String>::new());
}

const TABLE: &str = r#"{
  "version": "test-1",
  "models": { "echo-model-1": { "input": 2.0, "output": 10.0, "cacheRead": 0.5 } }
}"#;

fn usage(model: Option<&str>, input: u64, output: u64, cache_read: u64) -> AgentEvent {
    AgentEvent::TokenUsage {
        model: model.map(str::to_string),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: 0,
        cost_usd: None,
    }
}

/// Every SpendRecorded's spend in `events`.
fn spends(events: &[Event]) -> Vec<Spend> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::SpendRecorded { spend, .. } => Some(spend.clone()),
            _ => None,
        })
        .collect()
}

/// USD to cents, rounded to a hundredth of a cent (float noise aside).
fn cents(usd: Option<f64>) -> Option<f64> {
    usd.map(|usd| (usd * 10_000.0).round() / 100.0)
}

#[test]
fn usage_on_an_unresolved_default_is_unpriced_until_a_model_name_resolves_it() {
    let (mut core, _repo, worktree) = setup();
    std::fs::write(core.data_dir().join("price-table.json"), TABLE).unwrap();
    started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init(None));

    let held = core.agent_says(0, usage(None, 100_000, 20_000, 400_000));
    let held = spends(&held).pop().unwrap();
    assert_eq!((held.reported_usd, held.estimated_usd), (None, None), "unpriced, never $0");
    assert_eq!(held.by_model[0].model, None);

    // The first concrete Model name resolves Default: the usage so far is on it.
    let resolved = core.agent_says(0, usage(Some("echo-model-1-20261001"), 300_000, 50_000, 1_000_000));

    assert_eq!(
        system_lines(&resolved),
        vec!["Model: Default resolved to echo-model-1-20261001; earlier usage is priced on it".to_string()]
    );
    assert_eq!(last_facts(&resolved).model.as_deref(), Some("echo-model-1-20261001"));
    // 300k in * 2 + 50k out * 10 + 1M cache * 0.5 = 0.6 + 0.5 + 0.5: the running total on
    // that Model, superseding what was reported before it had a name (never added to it).
    let spend = spends(&resolved).pop().unwrap();
    assert_eq!(cents(spend.estimated_usd), Some(160.0));
    assert_eq!(spend.by_model.len(), 1);
    assert_eq!(spend.by_model[0].model.as_deref(), Some("echo-model-1-20261001"));

    let later = core.agent_says(0, usage(Some("echo-model-1-20261001"), 400_000, 50_000, 1_000_000));
    assert_eq!(system_lines(&later), Vec::<String>::new(), "resolved once");
}

#[test]
fn a_claude_code_default_its_init_does_not_name_is_resolved_by_its_usage() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix the README."));

    let events = core.agent_replays(0, &recorded_events("claude-code/default-not-reported"));

    assert_eq!(
        system_lines(&events),
        vec![
            "Model: Default (not reported)".to_string(),
            "Model: Default resolved to claude-opus-5-5; earlier usage is priced on it".to_string(),
        ]
    );
    assert_eq!(last_facts(&events).model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(spends(&events).pop().unwrap().reported_usd, Some(0.0412));
}

#[test]
fn a_default_resolved_by_a_relaunched_cli_reprices_the_usage_held_until_then() {
    let (mut core, _repo, worktree) = setup();
    std::fs::write(core.data_dir().join("price-table.json"), TABLE).unwrap();
    let id = started_session(&core.start_session(&worktree, "Fix it."));
    core.agent_says(0, init(None));
    core.agent_says(0, usage(None, 100_000, 20_000, 400_000));
    core.agent_says(0, AgentEvent::TurnComplete);
    core.agent_exits(0, Some(0), "");
    core.send(Command::SendPrompt { session_id: id, text: "Go on.".into() });

    let events = core.agent_says(1, init(Some("echo-model-1-20261001")));

    assert_eq!(
        system_lines(&events),
        vec!["Model: Default resolved to echo-model-1-20261001; earlier usage is priced on it".to_string()]
    );
    // 100k in * 2 + 20k out * 10 + 400k cache * 0.5 = 0.2 + 0.2 + 0.2
    let spend = spends(&events).pop().expect("the held usage is repriced");
    assert_eq!(cents(spend.estimated_usd), Some(60.0));
}

#[test]
fn a_reported_model_never_counts_as_resolving_a_chosen_one() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session_on(&worktree, PermissionMode::AskFirst, model("claude-sonnet-5-5"), "Fix it."));
    core.agent_says(0, init(None));

    let events = core.agent_says(0, usage(Some("claude-haiku-4-5"), 10, 10, 0));

    assert_eq!(system_lines(&events), Vec::<String>::new());
    assert_eq!(last_facts(&core.send(Command::Attach)).model.as_deref(), Some("claude-sonnet-5-5"));
}

#[test]
fn archived_sessions_keep_their_model_line() {
    let (mut core, _repo, worktree) = setup();
    let old = started_session(&core.start_session(&worktree, "One"));
    core.agent_says(0, init(Some("claude-opus-5-5")));
    core.send(Command::StopSession { session_id: old.clone() });
    core.send(Command::ReplaceSession {
        session_id: old.clone(),
        agent: AgentKind::ClaudeCode,
        permission_mode: PermissionMode::AskFirst,
        model: model("claude-sonnet-5-5"),
        prompt: "Two".into(),
    });
    assert_eq!(core.agents.launches()[1].model.as_deref(), Some("claude-sonnet-5-5"));

    let read = core.send(Command::ReadArchivedSession { session_id: old.clone() });

    let entries = read
        .iter()
        .find_map(|e| match e {
            Event::ArchivedTranscript { entries, .. } => Some(entries.clone()),
            _ => None,
        })
        .unwrap();
    assert!(entries.contains(&Entry::System { text: "Model: Default → claude-opus-5-5".into() }), "{entries:#?}");
}
