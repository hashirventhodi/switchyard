//! Codex sessions through seam 1: OpenAI Codex is chosen in New session like Claude Code,
//! runs in its own worktree with the mode and Grants translated for it, sits beside a
//! Claude Code session with its own status and requests, and resumes by its thread id.
mod harness;

use std::path::{Path, PathBuf};

use adapter_protocol::{Command as AgentCommand, Event as AgentEvent, Verdict as AgentVerdict};
use harness::{added_repository, recorded_events, started_session, Harness, TempRepo};
use switchyard_core::agents::SpentOn;
use switchyard_schema::{AgentKind, AgentSetupState, Command, Event, PermissionMode, SessionStatus, Verdict};

struct Setup {
    core: Harness,
    _repo: TempRepo,
    worktrees: Vec<PathBuf>,
}

/// A repository with `policy` committed (if any) and two hand-made worktrees.
fn setup_with(policy: Option<&str>) -> Setup {
    let repo = TempRepo::new("acme");
    if let Some(policy) = policy {
        std::fs::create_dir_all(repo.path.join(".switchyard")).unwrap();
        std::fs::write(repo.path.join(".switchyard/policy.yml"), policy).unwrap();
        harness::git(&repo.path, &["add", ".switchyard/policy.yml"]);
        harness::git(&repo.path, &["commit", "-q", "-m", "policy"]);
    }
    let worktrees = vec![repo.add_worktree("feat/billing"), repo.add_worktree("fix/readme")];
    let mut core = Harness::launch();
    let repo_id = added_repository(&core.add_repository(&repo.path)).id;
    core.acknowledge_policy(&repo_id);
    Setup { core, _repo: repo, worktrees }
}

fn setup() -> Setup {
    setup_with(None)
}

fn start(core: &mut Harness, worktree: &Path, agent: AgentKind, mode: PermissionMode, prompt: &str) -> String {
    started_session(&core.send(Command::StartSession {
        worktree_path: worktree.to_string_lossy().into_owned(),
        agent,
        permission_mode: mode,
        model: Default::default(),
        prompt: prompt.into(),
    }))
}

fn statuses(events: &[Event]) -> Vec<(String, SessionStatus)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::SessionStatusChanged { session_id, status } => Some((session_id.clone(), *status)),
            _ => None,
        })
        .collect()
}

fn thread_init(thread: &str, mode: adapter_protocol::PermissionMode, cli_mode: &str) -> AgentEvent {
    AgentEvent::SessionInit {
        external_session_id: thread.into(),
        permission_mode: mode,
        cli_permission_mode: cli_mode.into(),
        cli: adapter_protocol::CliInfo { name: "codex".into(), version: "0.144.1".into() },
        model: Some("gpt-5.6-terra".into()),
        mcp_servers: vec![],
    }
}

#[test]
fn codex_is_detected_beside_claude_code() {
    let mut core = Harness::launch();

    let agents = core
        .send(Command::Attach)
        .into_iter()
        .find_map(|e| match e {
            Event::AgentsChecked { agents } => Some(agents),
            _ => None,
        })
        .expect("agents are checked on attach");

    let codex = agents.iter().find(|a| a.agent == AgentKind::Codex).expect("Codex is listed");
    assert_eq!(codex.name, "Codex");
    assert_eq!(codex.state, AgentSetupState::Ready { version: "0.144.1".into() });
}

#[test]
fn a_codex_session_launches_codex_in_its_own_worktree_with_the_chosen_mode() {
    let mut s = setup();

    let id = start(&mut s.core, &s.worktrees[0], AgentKind::Codex, PermissionMode::AutoEdit, "Add a billing test");

    let launches = s.core.agents.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].agent, AgentKind::Codex);
    assert_eq!(launches[0].cwd, s.worktrees[0]);
    assert_eq!(launches[0].permission_mode, PermissionMode::AutoEdit);
    assert!(!launches[0].sandboxed);
    assert_eq!(s.core.agents.commands(0), vec![AgentCommand::SendText { text: "Add a billing test".into() }]);
    assert!(id.starts_with("session-"));
}

#[test]
fn a_recorded_codex_session_runs_and_finishes_a_turn() {
    let mut s = setup();
    let id = start(&mut s.core, &s.worktrees[0], AgentKind::Codex, PermissionMode::AskFirst, "Add a billing test");

    let events = s.core.agent_replays(0, &recorded_events("codex/first-session"));

    let facts = events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionFactsChanged { facts, .. } => Some(facts.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(facts.cli_version.as_deref(), Some("0.144.1"));
    assert_eq!(facts.cli_permission_mode.as_deref(), Some("untrusted / workspace-write"));
    assert_eq!(facts.mode_warning, None);
    assert_eq!(statuses(&events).first(), Some(&(id.clone(), SessionStatus::Waiting)));
    let tools: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { entry: switchyard_schema::Entry::Tool { summary, .. }, .. } => Some(summary.clone()),
            _ => None,
        })
        .collect();
    assert!(tools.contains(&"rg -n BillingQueue src".to_string()), "{tools:?}");
}

#[test]
fn two_agents_in_two_worktrees_keep_their_own_statuses_and_requests() {
    let mut s = setup();
    let claude = start(&mut s.core, &s.worktrees[0], AgentKind::ClaudeCode, PermissionMode::AskFirst, "Fix the docs");
    let codex = start(&mut s.core, &s.worktrees[1], AgentKind::Codex, PermissionMode::AskFirst, "Run the tests");
    let launches = s.core.agents.launches();
    assert_eq!((launches[0].agent, launches[1].agent), (AgentKind::ClaudeCode, AgentKind::Codex));

    // Codex asks to run a command: only its session needs the user.
    s.core.agent_says(1, thread_init("thread-1", adapter_protocol::PermissionMode::AskFirst, "untrusted / workspace-write"));
    let asked = s.core.agent_replays(1, &recorded_events("codex/permission-relay")[1..3]);
    assert_eq!(statuses(&asked), vec![(codex.clone(), SessionStatus::Waiting)]);
    // Claude Code asks too, independently.
    let claude_asks = s.core.agent_says(
        0,
        AgentEvent::PermissionRequest {
            request_id: "req-1".into(),
            request: adapter_protocol::RequestDetail::Command { command: "pnpm lint".into(), cwd: None },
            reason: None,
        },
    );
    assert_eq!(statuses(&claude_asks), vec![(claude.clone(), SessionStatus::Waiting)]);

    // Approving Codex's request (from the Dashboard) reaches Codex only, and moves only it.
    let answered = s.core.send(Command::AnswerRequest {
        session_id: codex.clone(),
        request_id: "0".into(),
        verdict: Verdict::AllowOnce,
    });

    assert_eq!(statuses(&answered), vec![(codex, SessionStatus::Running)]);
    assert_eq!(
        s.core.agents.commands(1).last(),
        Some(&AgentCommand::AnswerRequest { request_id: "0".into(), verdict: AgentVerdict::AllowOnce })
    );
    assert!(!s.core.agents.commands(0).iter().any(|c| matches!(c, AgentCommand::AnswerRequest { .. })));
}

const POLICY: &str = "\
permissions:
  allow:
    - command: pnpm *
    - network: registry.npmjs.org
    - mcp: { server: linear, tool: list_issues }
    - mcp: { server: docs, tool: '*' }
  deny:
    - command: git push *
    - mcp: { server: linear, tool: delete_issue }
";

#[test]
fn grants_reach_codex_as_mcp_tool_rules_and_commands_stay_with_switchyard() {
    let mut s = setup_with(Some(POLICY));

    start(&mut s.core, &s.worktrees[0], AgentKind::Codex, PermissionMode::AskFirst, "Triage the issues");

    let spec = &s.core.agents.launches()[0];
    // Codex has no per-invocation rule for commands or domains: Switchyard answers those
    // requests itself when Codex asks.
    assert_eq!(spec.allowed_tools, vec!["mcp:linear/list_issues".to_string(), "mcp:docs/*".into()]);
    assert_eq!(spec.disallowed_tools, vec!["mcp:linear/delete_issue".to_string()]);
}

#[test]
fn a_command_grant_answers_codexs_request_without_asking() {
    let mut s = setup_with(Some(POLICY));
    let id = start(&mut s.core, &s.worktrees[0], AgentKind::Codex, PermissionMode::AskFirst, "Run the tests");

    let events = s.core.agent_says(
        0,
        AgentEvent::PermissionRequest {
            request_id: "0".into(),
            request: adapter_protocol::RequestDetail::Command { command: "pnpm test".into(), cwd: None },
            reason: None,
        },
    );

    assert!(statuses(&events).is_empty(), "{events:#?}");
    assert_eq!(
        s.core.agents.commands(0).last(),
        Some(&AgentCommand::AnswerRequest { request_id: "0".into(), verdict: AgentVerdict::AllowOnce })
    );
    let _ = id;
}

#[test]
fn codex_starting_in_another_mode_than_asked_raises_the_mode_warning() {
    let mut s = setup();
    let id = start(&mut s.core, &s.worktrees[0], AgentKind::Codex, PermissionMode::FullAuto, "Go");

    let events = s.core.agent_replays(0, &recorded_events("codex/full-auto-downgraded"));

    let warning = events.iter().find_map(|e| match e {
        Event::SessionFactsChanged { session_id, facts } if *session_id == id => facts.mode_warning.clone(),
        _ => None,
    });
    assert_eq!(
        warning.as_deref(),
        Some("Asked for Full auto, but Codex started in Auto edit (on-request / workspace-write)")
    );
}

#[test]
fn a_codex_session_resumes_by_its_thread_id_after_a_relaunch() {
    let mut s = setup();
    let id = start(&mut s.core, &s.worktrees[0], AgentKind::Codex, PermissionMode::AskFirst, "Fix the README link");
    s.core.agent_says(0, thread_init("019a-thread", adapter_protocol::PermissionMode::AskFirst, "untrusted / workspace-write"));
    s.core.agent_says(0, AgentEvent::TurnComplete);

    let mut core = s.core.relaunch();
    core.send(Command::ResumeSession { session_id: id });

    let launches = core.agents.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].agent, AgentKind::Codex);
    assert_eq!(launches[0].cwd, s.worktrees[0]);
    assert_eq!(
        core.agents.commands(0),
        vec![
            AgentCommand::Resume { external_session_id: "019a-thread".into(), model: None },
            AgentCommand::SendText { text: "Continue where you left off.".into() },
        ]
    );
}

#[test]
fn a_relaunched_codex_agent_is_told_what_the_session_spent_on_each_model_so_far() {
    let mut s = setup();
    let id = start(&mut s.core, &s.worktrees[0], AgentKind::Codex, PermissionMode::AskFirst, "Fix the README link");
    s.core.agent_says(0, thread_init("019a-thread", adapter_protocol::PermissionMode::AskFirst, "untrusted / workspace-write"));
    let usage = |model: &str, input: u64| AgentEvent::TokenUsage {
        model: Some(model.into()),
        input_tokens: input,
        output_tokens: 300,
        cache_read_tokens: 4000,
        cache_write_tokens: 0,
        cost_usd: None,
    };
    s.core.agent_says(0, usage("gpt-5.6-terra", 6000));
    s.core.agent_says(0, usage("gpt-5.6-terra", 8000));
    s.core.agent_says(0, AgentEvent::TurnComplete);
    s.core.agent_exits(0, Some(0), "");

    s.core.send(Command::SendPrompt { session_id: id, text: "Go on.".into() });

    let terra = SpentOn {
        model: "gpt-5.6-terra".into(),
        input_tokens: 8000,
        output_tokens: 300,
        cache_read_tokens: 4000,
        cache_write_tokens: 0,
    };
    assert_eq!(s.core.agents.launches()[1].spent, vec![terra], "the latest running total on each Model");
    assert!(s.core.agents.launches()[0].spent.is_empty(), "nothing at start");
}

#[test]
fn every_recorded_codex_fixture_is_inside_this_builds_supported_range() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../adapter-protocol/fixtures/codex");
    for dir in std::fs::read_dir(fixtures).unwrap() {
        let meta = std::fs::read_to_string(dir.unwrap().path().join("meta.json")).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta).unwrap();
        let version = meta["cli"]["version"].as_str().unwrap().to_owned();
        let mut core = Harness::launch();
        core.probe.codex_prints(Some(&format!("codex-cli {version}")));
        let agents = core
            .send(Command::Attach)
            .into_iter()
            .find_map(|e| match e {
                Event::AgentsChecked { agents } => Some(agents),
                _ => None,
            })
            .unwrap();
        let codex = agents.into_iter().find(|a| a.agent == AgentKind::Codex).unwrap();
        assert_eq!(codex.state, AgentSetupState::Ready { version: version.clone() }, "{version} is outside the range");
    }
}

#[test]
fn a_codex_session_is_given_the_repositorys_mcp_servers_and_reports_their_state() {
    let repo = TempRepo::new("acme");
    std::fs::write(
        repo.path.join(".mcp.json"),
        r#"{"mcpServers":{"docs":{"command":"npx","args":["-y","@acme/docs-mcp"]},"search":{"type":"http","url":"https://search.example.com/mcp"}}}"#,
    )
    .unwrap();
    let worktree = repo.add_worktree("feat/docs");
    let mut core = Harness::launch();
    let repository_id = added_repository(&core.add_repository(&repo.path)).id;
    for name in ["docs", "search"] {
        core.send(Command::EnableMcpServer { repository_id: repository_id.clone(), name: name.into() });
    }
    start(&mut core, &worktree, AgentKind::Codex, PermissionMode::AskFirst, "Look up the billing docs.");
    let names: Vec<String> = core.agents.launches()[0].mcp_servers.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, vec!["docs".to_string(), "search".into()]);

    let events = core.agent_replays(0, &recorded_events("codex/mcp-servers"));

    let facts = events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionFactsChanged { facts, .. } => Some(facts.clone()),
            _ => None,
        })
        .unwrap();
    let states: Vec<(String, switchyard_schema::McpState, Option<String>)> =
        facts.mcp_servers.into_iter().map(|f| (f.name, f.state, f.error)).collect();
    assert_eq!(
        states,
        vec![
            ("docs".into(), switchyard_schema::McpState::Connected, None),
            ("search".into(), switchyard_schema::McpState::Failed, Some("HTTP 401 Unauthorized".into())),
        ]
    );
}

fn asks(id: &str, request: adapter_protocol::RequestDetail) -> AgentEvent {
    AgentEvent::PermissionRequest { request_id: id.into(), request, reason: None }
}

fn command(text: &str) -> adapter_protocol::RequestDetail {
    adapter_protocol::RequestDetail::Command { command: text.into(), cwd: None }
}

/// Codex in Auto edit asks for every command it does not trust (approval policy
/// `untrusted`), so Switchyard sees them all: it answers ordinary commands and edits by the
/// mode, and only dangerous ones reach the user.
#[test]
fn codex_auto_edit_lets_ordinary_commands_and_edits_run_but_a_dangerous_command_asks() {
    let mut s = setup();
    let wt = s.worktrees[0].clone();
    let id = start(&mut s.core, &wt, AgentKind::Codex, PermissionMode::AutoEdit, "Tidy the build.");
    s.core.agent_says(0, thread_init("thr-1", adapter_protocol::PermissionMode::AutoEdit, "untrusted / workspace-write"));

    let events = s.core.agent_says(0, asks("1", command("npm test")));
    assert!(statuses(&events).is_empty(), "no card: {events:#?}");
    assert!(
        events.iter().any(|e| matches!(e, Event::TranscriptEntry { entry: switchyard_schema::Entry::System { text }, .. }
            if text == "Allowed without asking: npm test (Auto edit)")),
        "{events:#?}"
    );
    let edit = adapter_protocol::RequestDetail::FileWrite { path: "src/build.ts".into(), diff: Some("+x".into()) };
    let events = s.core.agent_says(0, asks("2", edit));
    assert!(statuses(&events).is_empty(), "{events:#?}");
    let answers: Vec<_> = s
        .core
        .agents
        .commands(0)
        .into_iter()
        .filter(|c| matches!(c, AgentCommand::AnswerRequest { .. }))
        .collect();
    assert_eq!(
        answers,
        vec![
            AgentCommand::AnswerRequest { request_id: "1".into(), verdict: AgentVerdict::AllowOnce },
            AgentCommand::AnswerRequest { request_id: "2".into(), verdict: AgentVerdict::AllowOnce },
        ]
    );

    let events = s.core.agent_says(0, asks("3", command("rm -rf build")));
    assert_eq!(statuses(&events), vec![(id, SessionStatus::Waiting)], "a dangerous command needs the user");
}

#[test]
fn codex_ask_first_and_claude_auto_edit_still_ask_for_ordinary_commands() {
    let mut s = setup();
    let (a, b) = (s.worktrees[0].clone(), s.worktrees[1].clone());
    let codex = start(&mut s.core, &a, AgentKind::Codex, PermissionMode::AskFirst, "Tidy the build.");
    let claude = start(&mut s.core, &b, AgentKind::ClaudeCode, PermissionMode::AutoEdit, "Tidy the docs.");
    let events = s.core.agent_says(0, asks("1", command("npm test")));
    assert_eq!(statuses(&events), vec![(codex, SessionStatus::Waiting)]);
    let events = s.core.agent_says(1, asks("1", command("npm test")));
    assert_eq!(statuses(&events), vec![(claude, SessionStatus::Waiting)]);
}
