//! Permission modes through seam 1: the mode reaches the launcher (bypass only when the
//! repository's policy declares a sandbox), the core warns when the CLI started in another
//! mode than the one asked for, classifier denials are counted and never need the user,
//! and network, delete, migration and push commands prompt in Ask first and Auto edit
//! unless a grant names them explicitly.
mod harness;

use std::path::{Path, PathBuf};

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
