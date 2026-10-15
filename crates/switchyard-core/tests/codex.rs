//! Codex sessions through seam 1: OpenAI Codex is chosen in New session like Claude Code,
//! runs in its own worktree with the mode and Grants translated for it, sits beside a
//! Claude Code session with its own status and requests, and resumes by its thread id.
mod harness;

use std::path::{Path, PathBuf};

use adapter_protocol::{Command as AgentCommand, Event as AgentEvent, Verdict as AgentVerdict};
use harness::{added_repository, recorded_events, started_session, Harness, TempRepo};
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
    added_repository(&core.add_repository(&repo.path));
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
            AgentCommand::Resume { external_session_id: "019a-thread".into() },
            AgentCommand::SendText { text: "Continue where you left off.".into() },
        ]
    );
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
