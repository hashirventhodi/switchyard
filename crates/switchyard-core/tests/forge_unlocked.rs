//! The Forge never holds the core while `gh` or `git push` talks to the network: agent
//! output, shells and other commands carry on while a pull request opens, issues sync or
//! the poller waits on GitHub. A `gh` that never answers is given up on.

mod harness;

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

use harness::{added_repository, git, Harness, TempRepo};
use switchyard_core::agents::AgentOutput;
use switchyard_schema::{Command, Event};

struct Remote {
    repo: TempRepo,
    _bare: tempfile::TempDir,
    wt: PathBuf,
    core: Harness,
    repo_id: String,
}

fn with_remote(core: Harness) -> Remote {
    let mut core = core;
    let repo = TempRepo::new("acme");
    git(&repo.path, &["config", "user.name", "Test"]);
    git(&repo.path, &["config", "user.email", "test@example.com"]);
    let bare = tempfile::TempDir::new().unwrap();
    git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(&repo.path, &["remote", "add", "origin", bare.path().to_str().unwrap()]);
    git(&repo.path, &["push", "-q", "-u", "origin", "main"]);
    let wt = repo.add_worktree("feat/login");
    std::fs::write(wt.join("login.txt"), "form\n").unwrap();
    git(&wt, &["add", "login.txt"]);
    git(&wt, &["commit", "-q", "-m", "Add the login form"]);
    core.gh.install();
    core.send(Command::Attach);
    let repo_id = added_repository(&core.add_repository(&repo.path)).id;
    Remote { repo, _bare: bare, wt, core, repo_id }
}

fn path(p: &Path) -> String {
    p.to_str().unwrap().into()
}

/// While `gh <verb>` is held, `work` (run on `r`'s core from `command`'s thread) must not
/// stop the agent's output reaching the core: an agent event delivered meanwhile comes
/// back within seconds.
fn agent_output_flows_while(r: &mut Remote, verb: [&str; 2], command: Command) {
    let session = harness::started_session(&r.core.start_session(&r.wt, "Polish the form."));
    r.core.gh.hold(verb);
    let core = r.core.core();
    let agents = r.core.agents.clone();
    let gh = r.core.gh.clone();
    std::thread::scope(|s| {
        s.spawn(|| core.dispatch(command));
        gh.wait_until_held(verb);
        let (tx, rx) = channel();
        s.spawn(move || {
            agents.deliver(0, AgentOutput::Event(adapter_protocol::Event::TurnComplete));
            let _ = tx.send(());
        });
        let flowed = rx.recv_timeout(Duration::from_secs(10)).is_ok();
        gh.release(verb);
        assert!(flowed, "agent output waited for gh {} {} to finish", verb[0], verb[1]);
    });
    let events = r.core.drain();
    assert!(
        events.iter().any(|e| matches!(e, Event::AgentEventReceived { session_id, .. } if *session_id == session)),
        "{events:#?}"
    );
}

#[test]
fn agent_output_flows_while_a_pull_request_opens() {
    let mut r = with_remote(Harness::launch());
    let command = Command::OpenPullRequest { worktree_path: path(&r.wt) };
    agent_output_flows_while(&mut r, ["pr", "create"], command);
    assert_eq!(r.core.gh.calls_of(["pr", "create"]), 1);
}

#[test]
fn agent_output_flows_while_issues_sync() {
    let mut r = with_remote(Harness::launch());
    let command = Command::SyncTasks { repository_id: r.repo_id.clone() };
    agent_output_flows_while(&mut r, ["issue", "list"], command);
}

#[test]
fn agent_output_flows_while_gh_is_detected() {
    let mut r = with_remote(Harness::launch());
    agent_output_flows_while(&mut r, ["auth", "status"], Command::CheckForge);
}

#[test]
fn agent_output_flows_while_prune_merged_asks_the_forge() {
    let mut r = with_remote(Harness::launch());
    r.repo.add_worktree("feat/free"); // prune asks only about free worktrees
    let command = Command::PruneMerged { repository_id: r.repo_id.clone() };
    agent_output_flows_while(&mut r, ["pr", "list"], command);
}

#[test]
fn agent_output_flows_while_the_poller_waits_on_the_forge() {
    let mut r = with_remote(Harness::launch());
    let session = harness::started_session(&r.core.start_session(&r.wt, "Polish the form."));
    r.core.gh.hold(["pr", "list"]);
    let core = r.core.core();
    let agents = r.core.agents.clone();
    let gh = r.core.gh.clone();
    std::thread::scope(|s| {
        s.spawn(|| core.poll_forge());
        gh.wait_until_held(["pr", "list"]);
        let (tx, rx) = channel();
        s.spawn(move || {
            agents.deliver(0, AgentOutput::Event(adapter_protocol::Event::TurnComplete));
            let _ = tx.send(());
        });
        let flowed = rx.recv_timeout(Duration::from_secs(10)).is_ok();
        gh.release(["pr", "list"]);
        assert!(flowed, "agent output waited for the poll");
    });
    let events = r.core.drain();
    assert!(events.iter().any(|e| matches!(e, Event::AgentEventReceived { session_id, .. } if *session_id == session)));
}

#[test]
fn a_gh_that_never_answers_is_given_up_on() {
    let mut r = with_remote(Harness::launch_with_gh_timeout(Duration::from_millis(300)));
    r.core.gh.hold(["pr", "create"]);
    let events = r.core.send(Command::OpenPullRequest { worktree_path: path(&r.wt) });
    r.core.gh.release(["pr", "create"]);
    let reason = events
        .iter()
        .find_map(|e| match e {
            Event::CommandRefused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a refusal in {events:#?}"));
    assert!(reason.contains("did not answer"), "{reason}");
}
