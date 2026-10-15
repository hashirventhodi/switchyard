//! Permission modes through seam 1: the mode reaches the launcher (bypass only when the
//! repository's policy declares a sandbox), the core warns when the CLI started in another
//! mode than the one asked for, classifier denials are counted and never need the user,
//! and network, delete, migration and push commands prompt in Ask first and Auto edit
//! unless a grant names them explicitly.
mod harness;

use std::path::PathBuf;

use adapter_protocol::{
    CliInfo, Command as AgentCommand, Event as AgentEvent, PermissionMode as AgentMode, RequestDetail,
    Verdict as AgentVerdict,
};
use harness::{added_repository, started_session, Harness, TempRepo};
use switchyard_schema::{Command, Entry, Event, PermissionMode, RequestState, SessionFacts, SessionStatus};

struct Setup {
    core: Harness,
    _repo: TempRepo,
    worktree: PathBuf,
}

fn setup(policy: Option<&str>) -> Setup {
    let repo = TempRepo::new("acme");
    if let Some(policy) = policy {
        std::fs::create_dir_all(repo.path.join(".switchyard")).unwrap();
        std::fs::write(repo.path.join(".switchyard/policy.yml"), policy).unwrap();
        harness::git(&repo.path, &["add", ".switchyard/policy.yml"]);
        harness::git(&repo.path, &["commit", "-q", "-m", "policy"]);
    }
    let worktree = repo.add_worktree("feat/modes");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    Setup { core, _repo: repo, worktree }
}

fn start(s: &mut Setup, mode: PermissionMode) -> String {
    let path = s.worktree.clone();
    started_session(&s.core.start_session_in(&path, mode, "Ship it."))
}

fn command(id: &str, command: &str) -> AgentEvent {
    AgentEvent::PermissionRequest {
        request_id: id.into(),
        request: RequestDetail::Command { command: command.into(), cwd: None },
        reason: None,
    }
}

fn cards(events: &[Event]) -> Vec<(String, RequestState)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { entry: Entry::Request { request_id, state, .. }, .. } => {
                Some((request_id.clone(), *state))
            }
            _ => None,
        })
        .collect()
}

fn system_lines(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { entry: Entry::System { text }, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn status_changes(events: &[Event]) -> Vec<SessionStatus> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::SessionStatusChanged { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
}

fn refused(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CommandRefused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

fn last_facts(events: &[Event]) -> SessionFacts {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionFactsChanged { facts, .. } => Some(facts.clone()),
            _ => None,
        })
        .expect("facts in events")
}

const BROAD: &str = "permissions:\n  allow:\n    - command: git *\n    - command: pnpm *\n    - command: rm *\n";

/// Each request in turn on a fresh session in `mode` under the BROAD policy: true when it
/// became a pending card.
fn prompts(mode: PermissionMode, cmd: &str) -> bool {
    let mut s = setup(Some(BROAD));
    start(&mut s, mode);
    let events = s.core.agent_says(0, command("req-1", cmd));
    let carded = cards(&events) == vec![("req-1".to_string(), RequestState::Pending)];
    let allowed = system_lines(&events).iter().any(|l| l.starts_with("Allowed without asking"));
    assert!(carded != allowed, "{cmd}: exactly one of card or policy answer, got {events:#?}");
    carded
}

#[test]
fn dangerous_commands_prompt_in_ask_first_and_auto_edit_despite_a_wildcard_grant() {
    for mode in [PermissionMode::AskFirst, PermissionMode::AutoEdit] {
        for cmd in [
            "git push origin feat/modes",
            "pnpm db:migrate",
            "pnpm exec curl https://example.com",
            "git clean -fdx",
        ] {
            assert!(prompts(mode, cmd), "{mode:?}: {cmd} must prompt");
        }
        // Not dangerous: the wildcard answers it.
        assert!(!prompts(mode, "git status"), "{mode:?}: git status is granted");
        // Explicitly granted: the grant names the dangerous command itself.
        assert!(!prompts(mode, "rm -rf build"), "{mode:?}: rm * names rm");
    }
}

#[test]
fn full_auto_lets_the_policy_answer_dangerous_commands() {
    assert!(!prompts(PermissionMode::FullAuto, "git push origin feat/modes"));
}

#[test]
fn an_explicit_grant_for_push_answers_push_in_ask_first() {
    let mut s = setup(Some("permissions:\n  allow:\n    - command: git push *\n"));
    start(&mut s, PermissionMode::AskFirst);
    let events = s.core.agent_says(0, command("req-1", "git push origin feat/modes"));
    assert_eq!(cards(&events), vec![]);
    assert_eq!(
        system_lines(&events),
        vec!["Allowed without asking: git push origin feat/modes (repository policy git push *)".to_string()]
    );
    assert_eq!(
        s.core.agents.commands(0).last(),
        Some(&AgentCommand::AnswerRequest { request_id: "req-1".into(), verdict: AgentVerdict::AllowOnce })
    );
}

fn init(mode: AgentMode, cli_mode: &str) -> AgentEvent {
    AgentEvent::SessionInit {
        external_session_id: "ext-1".into(),
        permission_mode: mode,
        cli_permission_mode: cli_mode.into(),
        cli: CliInfo { name: "claude-code".into(), version: "2.1.300".into() },
        model: Some("claude-opus-5-5".into()),
        mcp_servers: vec![],
    }
}

#[test]
fn init_in_another_mode_than_requested_warns_on_the_facts() {
    let mut s = setup(None);
    start(&mut s, PermissionMode::FullAuto);
    let events = s.core.agent_says(0, init(AgentMode::AskFirst, "default"));
    assert_eq!(
        last_facts(&events).mode_warning.as_deref(),
        Some("Asked for Full auto, but Claude Code started in Ask first (default)")
    );
    assert_eq!(status_changes(&events), vec![]);
}

#[test]
fn init_in_the_requested_mode_has_no_warning() {
    let mut s = setup(None);
    start(&mut s, PermissionMode::AutoEdit);
    let events = s.core.agent_says(0, init(AgentMode::AutoEdit, "acceptEdits"));
    assert_eq!(last_facts(&events).mode_warning, None);
    assert_eq!(last_facts(&events).cli_permission_mode.as_deref(), Some("acceptEdits"));
}

#[test]
fn classifier_denials_are_counted_and_noted_but_never_need_you() {
    let mut s = setup(None);
    start(&mut s, PermissionMode::FullAuto);
    s.core.agent_says(0, init(AgentMode::FullAuto, "auto"));
    let denial = || AgentEvent::ClassifierDenial {
        tool: "Bash".into(),
        detail: "rm -rf /tmp/elsewhere".into(),
        reason: Some("destructive outside the worktree".into()),
    };
    s.core.agent_says(0, denial());
    let events = s.core.agent_says(0, denial());
    assert_eq!(last_facts(&events).classifier_denials, 2);
    assert_eq!(
        system_lines(&events),
        vec!["Denied by the classifier: Bash rm -rf /tmp/elsewhere (destructive outside the worktree)".to_string()]
    );
    assert_eq!(cards(&events), vec![]);
    assert_eq!(status_changes(&events), vec![], "never Needs you");
}

#[test]
fn full_auto_bypasses_only_where_the_policy_declares_a_sandbox() {
    let mut s = setup(Some("permissions:\n  sandbox: true\n"));
    start(&mut s, PermissionMode::FullAuto);
    let launch = s.core.agents.launches().remove(0);
    assert!(launch.sandboxed);
    assert_eq!(switchyard_core::agents::protocol_mode(launch.permission_mode, launch.sandboxed), AgentMode::Bypass);
    let events = s.core.agent_says(0, init(AgentMode::Bypass, "bypassPermissions"));
    assert_eq!(last_facts(&events).mode_warning, None);

    let mut s = setup(None);
    start(&mut s, PermissionMode::FullAuto);
    let launch = s.core.agents.launches().remove(0);
    assert!(!launch.sandboxed);
    assert_eq!(switchyard_core::agents::protocol_mode(launch.permission_mode, launch.sandboxed), AgentMode::FullAuto);
}

#[test]
fn every_mode_maps_to_its_protocol_mode() {
    use switchyard_core::agents::protocol_mode;
    assert_eq!(protocol_mode(PermissionMode::Explore, true), AgentMode::Explore);
    assert_eq!(protocol_mode(PermissionMode::AskFirst, true), AgentMode::AskFirst);
    assert_eq!(protocol_mode(PermissionMode::AutoEdit, true), AgentMode::AutoEdit);
    assert_eq!(protocol_mode(PermissionMode::FullAuto, false), AgentMode::FullAuto);
}

fn set_mode(s: &mut Setup, id: &str, mode: PermissionMode) -> Vec<Event> {
    s.core.send(Command::SetPermissionMode { session_id: id.into(), permission_mode: mode })
}

#[test]
fn the_composer_switches_a_running_session_to_another_mode() {
    let mut s = setup(None);
    let id = start(&mut s, PermissionMode::FullAuto);
    s.core.agent_says(0, init(AgentMode::AskFirst, "default"));

    let events = set_mode(&mut s, &id, PermissionMode::AutoEdit);

    assert_eq!(refused(&events), Vec::<String>::new());
    assert!(events.contains(&Event::SessionModeChanged { session_id: id.clone(), permission_mode: PermissionMode::AutoEdit }));
    assert_eq!(system_lines(&events), vec!["Permission mode: Auto edit (was Full auto).".to_string()]);
    assert_eq!(last_facts(&events).mode_warning, None, "the old warning is about the old mode");
    assert_eq!(
        s.core.agents.commands(0).last(),
        Some(&AgentCommand::SetPermissionMode { permission_mode: AgentMode::AutoEdit })
    );
    assert_eq!(status_changes(&events), vec![]);
}

#[test]
fn the_ceiling_refuses_a_switch_above_it() {
    let mut s = setup(Some("permissions:\n  ceiling: auto_edit\n"));
    let id = start(&mut s, PermissionMode::AskFirst);
    let sent = s.core.agents.commands(0).len();

    let events = set_mode(&mut s, &id, PermissionMode::FullAuto);

    assert_eq!(refused(&events), vec!["This repository's policy allows at most Auto edit".to_string()]);
    assert_eq!(s.core.agents.commands(0).len(), sent, "nothing reaches the agent");
    // And at start.
    let mut s = setup(Some("permissions:\n  ceiling: auto_edit\n"));
    let path = s.worktree.clone();
    let events = s.core.start_session_in(&path, PermissionMode::FullAuto, "Ship it.");
    assert_eq!(refused(&events), vec!["This repository's policy allows at most Auto edit".to_string()]);
}

#[test]
fn explore_is_chosen_at_start_and_never_switched_to_or_from() {
    let mut s = setup(None);
    let id = start(&mut s, PermissionMode::AskFirst);
    assert_eq!(
        refused(&set_mode(&mut s, &id, PermissionMode::Explore)),
        vec!["Explore is chosen when a session starts: start an Explore session instead".to_string()]
    );
    let mut s = setup(None);
    let id = start(&mut s, PermissionMode::Explore);
    assert_eq!(
        refused(&set_mode(&mut s, &id, PermissionMode::AutoEdit)),
        vec!["An Explore session stays in Explore: start a new session to make changes".to_string()]
    );
}

#[test]
fn a_resumed_session_relaunches_in_its_switched_mode() {
    let mut s = setup(None);
    let id = start(&mut s, PermissionMode::AskFirst);
    s.core.agent_says(0, init(AgentMode::AskFirst, "default"));
    set_mode(&mut s, &id, PermissionMode::AutoEdit);

    s.core = s.core.relaunch();
    let events = s.core.send(Command::ResumeSession { session_id: id });

    assert_eq!(refused(&events), Vec::<String>::new());
    let launch = s.core.agents.launches().remove(0);
    assert_eq!(launch.permission_mode, PermissionMode::AutoEdit);
    assert!(!launch.sandboxed);
    let events = s.core.agent_says(0, init(AgentMode::AutoEdit, "acceptEdits"));
    assert_eq!(last_facts(&events).mode_warning, None);
}

#[test]
fn a_resumed_sandboxed_session_relaunches_into_bypass_and_still_warns_on_another_mode() {
    let mut s = setup(Some("permissions:\n  sandbox: true\n"));
    let id = start(&mut s, PermissionMode::FullAuto);
    s.core.agent_says(0, init(AgentMode::Bypass, "bypassPermissions"));

    s.core = s.core.relaunch();
    s.core.send(Command::ResumeSession { session_id: id });

    let launch = s.core.agents.launches().remove(0);
    assert_eq!(launch.permission_mode, PermissionMode::FullAuto);
    assert!(launch.sandboxed);
    let events = s.core.agent_says(0, init(AgentMode::AskFirst, "default"));
    assert_eq!(
        last_facts(&events).mode_warning.as_deref(),
        Some("Asked for Full auto (sandbox bypass), but Claude Code started in Ask first (default)")
    );
}

fn always(s: &mut Setup, session: &str, request_id: &str, pattern: Option<&str>) -> Vec<Event> {
    s.core.send(Command::AnswerRequest {
        session_id: session.into(),
        request_id: request_id.into(),
        verdict: switchyard_schema::Verdict::AllowAlways { pattern: pattern.map(str::to_owned) },
    })
}

fn always_allowed(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TranscriptEntry { entry: Entry::Request { state: RequestState::AlwaysAllowed, answer, .. }, .. } => {
                answer.clone()
            }
            _ => None,
        })
        .collect()
}

/// Always allow on a dangerous command grants a pattern that names it, so the grant the
/// card promises is the one that answers it next time.
#[test]
fn always_allow_on_a_dangerous_command_grants_a_pattern_that_names_it_and_is_honoured() {
    let mut s = setup(None);
    let session = start(&mut s, PermissionMode::AskFirst);
    s.core.agent_says(0, command("req-1", "npm run db:migrate"));
    let events = always(&mut s, &session, "req-1", None);
    assert_eq!(always_allowed(&events), vec!["npm run db:migrate *".to_string()]);

    let events = s.core.agent_says(0, command("req-2", "npm run db:migrate"));
    assert_eq!(cards(&events), vec![], "the grant answers it: {events:#?}");
    assert_eq!(
        system_lines(&events),
        vec!["Allowed without asking: npm run db:migrate (your grant npm run db:migrate *)".to_string()]
    );
}

#[test]
fn a_grant_the_dangerous_guard_would_ignore_is_refused_rather_than_promised() {
    let mut s = setup(None);
    let session = start(&mut s, PermissionMode::AutoEdit);
    s.core.agent_says(0, command("req-1", "npm run db:migrate"));
    let events = always(&mut s, &session, "req-1", Some("npm run *"));
    assert_eq!(
        refused(&events),
        vec!["npm run * would not answer npm run db:migrate: below Full auto a migration command is granted only by a pattern that names it, such as npm run db:migrate *".to_string()]
    );
    assert_eq!(always_allowed(&events), Vec::<String>::new());
}
