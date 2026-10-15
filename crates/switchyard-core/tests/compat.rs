//! Agent detection and the compat log: before the first session Switchyard says whether
//! each agent CLI is installed and inside the version range this build was recorded
//! against, refuses to start one that is not, and records every adapter failure locally.

mod harness;

use harness::{recorded_events, started_session, Harness, TempRepo, NOW_MS};
use switchyard_schema::{AgentKind, AgentSetup, AgentSetupState, Command, Entry, Event, Link};

/// A repository with one free linked worktree, added to a fresh core.
fn setup() -> (Harness, TempRepo, std::path::PathBuf) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    (core, repo, worktree)
}

fn agents_checked(events: &[Event]) -> Vec<AgentSetup> {
    let found: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::AgentsChecked { agents } => Some(agents.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(found.len(), 1, "expected exactly one AgentsChecked in {events:#?}");
    found.into_iter().next().unwrap()
}

fn claude_state(events: &[Event]) -> AgentSetupState {
    let agents = agents_checked(events);
    let claude = agents.iter().find(|a| a.agent == AgentKind::ClaudeCode).expect("Claude Code is listed");
    assert_eq!(claude.name, "Claude Code");
    claude.state.clone()
}

fn refusal(events: &[Event]) -> String {
    match events {
        [Event::CommandRefused { command, reason }] if command == "startSession" => reason.clone(),
        other => panic!("expected a single startSession refusal, got {other:#?}"),
    }
}

const RELEASES: &str = "https://github.com/hashirventhodi/switchyard/releases";

#[test]
fn attaching_reports_an_installed_supported_cli_as_ready() {
    let mut core = Harness::launch();

    let events = core.send(Command::Attach);

    assert_eq!(claude_state(&events), AgentSetupState::Ready { version: "2.1.300".into() });
}

#[test]
fn a_cli_newer_than_this_build_blocks_new_sessions_with_an_update_link() {
    let (mut core, _repo, worktree) = setup();
    core.probe.cli_prints(Some("2.2.0 (Claude Code)"));

    let state = claude_state(&core.send(Command::Attach));
    let reason = refusal(&core.start_session(&worktree, "Fix the README"));

    let AgentSetupState::Unsupported { version, message, link } = state else { panic!("{state:?}") };
    assert_eq!(version.as_deref(), Some("2.2.0"));
    assert!(message.starts_with("Claude Code 2.2.0 is not yet supported by this build of Switchyard"), "{message}");
    assert_eq!(link, Some(Link { label: "Get the latest Switchyard".into(), url: RELEASES.into() }));
    assert_eq!(reason, message);
    assert!(core.agents.launches().is_empty());
}

#[test]
fn a_cli_older_than_this_build_blocks_new_sessions_and_asks_to_update_the_cli() {
    let (mut core, _repo, worktree) = setup();
    core.probe.cli_prints(Some("2.0.14 (Claude Code)"));

    let state = claude_state(&core.send(Command::Attach));
    let reason = refusal(&core.start_session(&worktree, "Fix the README"));

    let AgentSetupState::Unsupported { message, link, .. } = state else { panic!("{state:?}") };
    assert!(message.starts_with("Claude Code 2.0.14 is older than this build of Switchyard supports"), "{message}");
    assert!(message.contains("claude update"), "{message}");
    assert_eq!(link.map(|l| l.label), Some("How to update Claude Code".to_string()));
    assert_eq!(reason, message);
}

#[test]
fn an_unreadable_cli_version_blocks_new_sessions() {
    let (mut core, _repo, worktree) = setup();
    core.probe.cli_prints(Some("claude: something went wrong"));

    let state = claude_state(&core.send(Command::Attach));
    let reason = refusal(&core.start_session(&worktree, "Fix the README"));

    let AgentSetupState::Unsupported { version: None, message, .. } = state else { panic!("{state:?}") };
    assert!(message.contains("could not read the Claude Code version"), "{message}");
    assert_eq!(reason, message);
}

#[test]
fn a_missing_cli_is_a_setup_state_with_its_install_hint() {
    let (mut core, _repo, worktree) = setup();
    core.probe.cli_prints(None);

    let state = claude_state(&core.send(Command::Attach));
    let reason = refusal(&core.start_session(&worktree, "Fix the README"));

    let AgentSetupState::Missing { install_hint, link } = state else { panic!("{state:?}") };
    assert_eq!(install_hint, "curl -fsSL https://claude.ai/install.sh | bash");
    assert_eq!(link.url, "https://code.claude.com/docs/en/setup");
    assert!(reason.starts_with("Claude Code is not installed"), "{reason}");
    assert!(reason.contains(&install_hint), "{reason}");
}

#[test]
fn detection_runs_once_and_check_agents_runs_it_again() {
    let (mut core, _repo, worktree) = setup();
    core.probe.cli_prints(None);
    core.send(Command::Attach);
    core.send(Command::Attach);
    assert_eq!(core.probe.probes(), 1);

    // The user installs Claude Code and asks Switchyard to look again.
    core.probe.cli_prints(Some("2.1.301 (Claude Code)"));
    let state = claude_state(&core.send(Command::CheckAgents));
    let events = core.start_session(&worktree, "Fix the README");

    assert_eq!(state, AgentSetupState::Ready { version: "2.1.301".into() });
    started_session(&events);
    assert_eq!(core.probe.probes(), 2);
}

#[test]
fn starting_a_session_before_any_attach_detects_first() {
    let (mut core, _repo, worktree) = setup();
    core.probe.cli_prints(Some("9.0.0"));

    let reason = refusal(&core.start_session(&worktree, "Fix the README"));

    assert!(reason.contains("not yet supported"), "{reason}");
}

#[test]
fn an_adapter_speaking_another_protocol_blocks_sessions_and_is_logged() {
    let (mut core, _repo, worktree) = setup();
    core.probe.adapter_implements("^0.2");

    let state = claude_state(&core.send(Command::Attach));
    refusal(&core.start_session(&worktree, "Fix the README"));

    let AgentSetupState::Unsupported { message, .. } = state else { panic!("{state:?}") };
    assert!(message.contains("protocol"), "{message}");
    let log = core.compat_log();
    assert_eq!(log.len(), 1, "{log:#?}");
    assert_eq!(log[0]["failure_class"], "protocol_mismatch");
    assert_eq!(log[0]["cli_name"], "claude-code");
    assert_eq!(log[0]["cli_version"], "2.1.300");
    assert_eq!(log[0]["protocol_version"], "0.1.0");
    assert_eq!(log[0]["adapter_implements"], "^0.2");
}

#[test]
fn an_adapter_that_cannot_start_blocks_sessions_and_is_logged() {
    let mut core = Harness::launch();
    core.probe.adapter_fails("cannot run node: No such file or directory");

    let state = claude_state(&core.send(Command::Attach));

    let AgentSetupState::Unsupported { message, .. } = state else { panic!("{state:?}") };
    assert!(message.contains("cannot run node"), "{message}");
    let log = core.compat_log();
    assert_eq!(log.len(), 1, "{log:#?}");
    assert_eq!(log[0]["failure_class"], "spawn_failure");
    assert!(log[0]["detail"].as_str().unwrap().contains("cannot run node"));
}

#[test]
fn a_failed_launch_is_logged_with_the_cli_and_protocol_versions() {
    let (mut core, _repo, worktree) = setup();
    core.agents.refuse_next_launch("the sidecar script is missing");

    refusal(&core.start_session(&worktree, "Fix the README"));

    let log = core.compat_log();
    assert_eq!(log.len(), 1, "{log:#?}");
    let record = &log[0];
    assert_eq!(record["failure_class"], "spawn_failure");
    assert_eq!(record["agent"], "claude-code");
    assert_eq!(record["cli_name"], "claude-code");
    assert_eq!(record["cli_version"], "2.1.300");
    assert_eq!(record["protocol_version"], "0.1.0");
    assert_eq!(record["adapter_implements"], "^0.1");
    assert_eq!(record["at_ms"], NOW_MS);
    assert!(record["detail"].as_str().unwrap().contains("the sidecar script is missing"));
}

#[test]
fn an_agent_that_exits_before_it_starts_is_logged_as_a_spawn_failure() {
    let (mut core, _repo, worktree) = setup();
    core.start_session(&worktree, "Fix the README");

    core.agent_exits(0, Some(1), "Error: claude not found");

    let log = core.compat_log();
    assert_eq!(log.len(), 1, "{log:#?}");
    assert_eq!(log[0]["failure_class"], "spawn_failure");
    assert!(log[0]["detail"].as_str().unwrap().contains("claude not found"));
}

#[test]
fn an_agent_that_exits_after_starting_is_not_a_compat_failure() {
    let (mut core, _repo, worktree) = setup();
    core.start_session(&worktree, "Fix the README");
    core.agent_says(0, recorded_events("claude-code/first-session")[0].clone());

    core.agent_exits(0, Some(1), "boom");

    assert!(core.compat_log().is_empty());
}

#[test]
fn unreadable_agent_output_is_logged_as_a_parse_error_or_a_protocol_mismatch() {
    let (mut core, _repo, worktree) = setup();
    core.start_session(&worktree, "Fix the README");
    core.agent_says(0, recorded_events("claude-code/first-session")[0].clone());

    let events = core.agent_writes(0, "not json at all");
    core.agent_writes(0, r#"{"type":"agent_text"}"#);
    core.agent_writes(0, r#"{"type":"telepathy","thought":"hi"}"#);

    // The session carries on; the transcript notes the bad line.
    assert!(
        matches!(&events[..], [Event::TranscriptEntry { entry: Entry::System { text }, .. }] if text.contains("not json at all")),
        "{events:#?}"
    );
    let log = core.compat_log();
    let classes: Vec<_> = log.iter().map(|r| r["failure_class"].as_str().unwrap().to_owned()).collect();
    assert_eq!(classes, vec!["parse_error", "parse_error", "protocol_mismatch"]);
    // The CLI version is the one the agent reported at init.
    assert_eq!(log[0]["cli_version"], "2.1.300");
}

#[test]
fn the_compat_log_is_appended_across_relaunches() {
    let (mut core, _repo, worktree) = setup();
    core.agents.refuse_next_launch("first");
    core.start_session(&worktree, "Fix the README");
    let mut core = core.relaunch();
    core.agents.refuse_next_launch("second");

    core.start_session(&worktree, "Fix the README");

    let details: Vec<_> = core.compat_log().iter().map(|r| r["detail"].as_str().unwrap().to_owned()).collect();
    assert_eq!(details.len(), 2, "{details:?}");
    assert!(details[0].contains("first") && details[1].contains("second"), "{details:?}");
}

#[test]
fn every_recorded_claude_code_fixture_is_inside_this_builds_supported_range() {
    // A new CLI version means new recordings and an updated range: the two must agree.
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../adapter-protocol/fixtures/claude-code");
    for dir in std::fs::read_dir(fixtures).unwrap() {
        let meta = std::fs::read_to_string(dir.unwrap().path().join("meta.json")).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta).unwrap();
        let version = meta["cli"]["version"].as_str().unwrap().to_owned();
        let mut core = Harness::launch();
        core.probe.cli_prints(Some(&version));
        assert_eq!(
            claude_state(&core.send(Command::Attach)),
            AgentSetupState::Ready { version: version.clone() },
            "the fixture recorded against {version} is outside the supported range"
        );
    }
}
