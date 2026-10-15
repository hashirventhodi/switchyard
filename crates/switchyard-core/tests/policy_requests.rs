//! Committed policy is a request (ADR-0006): what a repository's `.switchyard/policy.yml`
//! takes away applies at once, what it adds applies only once the user acknowledged that
//! exact content, and a change asks again.
mod harness;

use std::path::PathBuf;

use adapter_protocol::{Event as AgentEvent, RequestDetail};
use harness::{added_repository, started_session, Harness, TempRepo};
use switchyard_schema::{Command, Entry, Event, PermissionMode, Policy, PolicyRequest, RequestState};

struct Setup {
    core: Harness,
    repo: TempRepo,
    repo_id: String,
    worktree: PathBuf,
}

fn setup(policy: &str) -> Setup {
    let repo = TempRepo::new("acme");
    write_policy(&repo, policy);
    let worktree = repo.add_worktree("feat/policy");
    let mut core = Harness::launch();
    let repo_id = added_repository(&core.add_repository(&repo.path)).id;
    Setup { core, repo, repo_id, worktree }
}

fn write_policy(repo: &TempRepo, policy: &str) {
    std::fs::create_dir_all(repo.path.join(".switchyard")).unwrap();
    std::fs::write(repo.path.join(".switchyard/policy.yml"), policy).unwrap();
}

impl Setup {
    fn policy(&mut self) -> Policy {
        let events = self.core.send(Command::ReadPolicy { repository_id: self.repo_id.clone() });
        events
            .into_iter()
            .find_map(|e| match e {
                Event::PolicyChanged { policy, .. } => Some(policy),
                _ => None,
            })
            .expect("policyChanged")
    }

    fn request(&mut self) -> PolicyRequest {
        self.policy().request.expect("a pending request")
    }

    fn acknowledge(&mut self, hash: &str) -> Vec<Event> {
        self.core.send(Command::AcknowledgePolicy { repository_id: self.repo_id.clone(), hash: hash.into() })
    }

    fn start(&mut self, mode: PermissionMode) -> String {
        let path = self.worktree.clone();
        started_session(&self.core.start_session_in(&path, mode, "Go."))
    }
}

fn asks(command: &str) -> AgentEvent {
    AgentEvent::PermissionRequest {
        request_id: "req-1".into(),
        request: RequestDetail::Command { command: command.into(), cwd: None },
        reason: None,
    }
}

fn carded(events: &[Event]) -> bool {
    events.iter().any(|e| matches!(e, Event::TranscriptEntry { entry: Entry::Request { state: RequestState::Pending, .. }, .. }))
}

fn refusals(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CommandRefused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

const ALLOWS: &str = "permissions:\n  allow:\n    - command: pnpm *\n  deny:\n    - command: git push *\n";

#[test]
fn a_committed_allow_answers_nothing_until_acknowledged() {
    let mut s = setup(ALLOWS);
    let policy = s.policy();
    let request = policy.request.clone().expect("the allow is a request");
    assert_eq!(request.allow.len(), 1, "{request:?}");
    assert!(policy.grants.iter().all(|g| g.id != "repo-allow-1"), "not applied: {:?}", policy.grants);

    s.start(PermissionMode::AskFirst);
    assert!(carded(&s.core.agent_says(0, asks("pnpm test"))), "unacknowledged: asks");

    s.acknowledge(&request.hash);
    assert_eq!(s.policy().request, None);
    s.core.send(Command::StopSession { session_id: "session-1".into() });
    let other = s.repo.add_worktree("feat/two");
    started_session(&s.core.start_session_in(&other, PermissionMode::AskFirst, "Again."));
    let events = s.core.agent_says(1, asks("pnpm test"));
    assert!(!carded(&events), "acknowledged: the allow answers it: {events:#?}");
}

#[test]
fn a_committed_denial_applies_at_once() {
    let mut s = setup(ALLOWS);
    let policy = s.policy();
    assert!(policy.grants.iter().any(|g| g.id == "repo-deny-1"));
    s.start(PermissionMode::AskFirst);
    let events = s.core.agent_says(0, asks("git push origin main"));
    assert!(!carded(&events));
    assert!(events.iter().any(|e| matches!(e, Event::TranscriptEntry { entry: Entry::System { text }, .. }
        if text.starts_with("Denied without asking"))), "{events:#?}");
}

#[test]
fn a_changed_request_asks_again_but_a_new_denial_does_not() {
    let mut s = setup(ALLOWS);
    let first = s.request().hash;
    s.acknowledge(&first);

    write_policy(&s.repo, &format!("{ALLOWS}    - command: rm -rf /\n"));
    assert_eq!(s.policy().request, None, "a new denial needs no acknowledgement");

    write_policy(&s.repo, "permissions:\n  allow:\n    - command: pnpm *\n    - command: curl *\n");
    let again = s.request();
    assert_ne!(again.hash, first);
    assert_eq!(again.allow.len(), 2);
}

#[test]
fn a_stale_acknowledgement_is_refused() {
    let mut s = setup(ALLOWS);
    let events = s.acknowledge("0000");
    assert_eq!(
        refusals(&events),
        vec!["The repository's policy changed since you read it; read it again before acknowledging".to_string()]
    );
    let mut none = setup("permissions:\n  deny:\n    - command: git push *\n");
    let events = none.acknowledge("0000");
    assert_eq!(refusals(&events), vec!["The repository's policy requests nothing to acknowledge".to_string()]);
}

#[test]
fn a_declared_sandbox_gives_full_auto_no_bypass_until_acknowledged_and_says_it_cannot_be_verified() {
    let mut s = setup("permissions:\n  sandbox: true\n");
    let request = s.request();
    assert!(request.sandbox);
    assert_eq!(request.sandbox_evidence, None, "the harness detects no container or VM");

    s.start(PermissionMode::FullAuto);
    assert!(!s.core.agents.launches()[0].sandboxed);

    s.acknowledge(&request.hash);
    s.core.send(Command::StopSession { session_id: "session-1".into() });
    let other = s.repo.add_worktree("feat/two");
    started_session(&s.core.start_session_in(&other, PermissionMode::FullAuto, "Again."));
    assert!(s.core.agents.launches()[1].sandboxed);
}

#[test]
fn a_committed_default_mode_above_ask_first_waits_for_acknowledgement() {
    let mut s = setup("permissions:\n  default_mode: full_auto\n");
    let policy = s.policy();
    assert_eq!(policy.default_mode, Some(PermissionMode::AskFirst));
    assert_eq!(policy.request.as_ref().and_then(|r| r.default_mode), Some(PermissionMode::FullAuto));
    let hash = policy.request.unwrap().hash;
    s.acknowledge(&hash);
    assert_eq!(s.policy().default_mode, Some(PermissionMode::FullAuto));
}

#[test]
fn a_committed_worktree_root_is_used_only_once_acknowledged() {
    let mut s = setup("worktrees:\n  root: ../elsewhere\n");
    let events = s.core.send(Command::CreateWorktree { repository_id: s.repo_id.clone(), branch: "feat/a".into() });
    let made = events
        .iter()
        .find_map(|e| match e {
            Event::WorktreeAdded { worktree, .. } => Some(PathBuf::from(&worktree.path)),
            _ => None,
        })
        .expect("created");
    assert!(made.starts_with(s.core.worktree_root()), "{made:?}");

    let hash = s.request().hash;
    s.acknowledge(&hash);
    let events = s.core.send(Command::CreateWorktree { repository_id: s.repo_id.clone(), branch: "feat/b".into() });
    let made = events
        .iter()
        .find_map(|e| match e {
            Event::WorktreeAdded { worktree, .. } => Some(PathBuf::from(&worktree.path)),
            _ => None,
        })
        .expect("created");
    assert!(made.to_string_lossy().contains("elsewhere"), "{made:?}");
}

#[test]
fn a_session_started_with_a_pending_request_says_what_does_not_apply() {
    let mut s = setup(ALLOWS);
    let path = s.worktree.clone();
    let events = s.core.start_session_in(&path, PermissionMode::AskFirst, "Go.");
    assert!(
        events.iter().any(|e| matches!(e, Event::TranscriptEntry { entry: Entry::System { text }, .. }
            if text.contains("acknowledge it in Context"))),
        "{events:#?}"
    );
}
