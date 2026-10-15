//! The Forge through seam 1: `gh` detection, Open pull request, polling, Merged cleanup,
//! Prune merged and Local merge. Repositories are real temporary git repositories with a
//! local bare remote; `gh` is a fake script (see `harness/gh.rs`). Nothing touches GitHub.

mod harness;

use std::path::{Path, PathBuf};

use harness::gh::gh_pr;
use harness::{added_repository, git, Harness, TempRepo, NOW_MS};
use serde_json::json;
use switchyard_schema::{
    CheckStatus, ChecksState, Command, Event, ForgeSetup, ForgeSetupState, PullRequest, PullRequestState, ReviewState,
};

fn forge_checked(events: &[Event]) -> ForgeSetup {
    let found: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::ForgeChecked { forge } => Some(forge.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(found.len(), 1, "expected exactly one ForgeChecked in {events:#?}");
    found.into_iter().next().unwrap()
}

#[test]
fn a_missing_gh_is_a_setup_state_on_attach() {
    let mut core = Harness::launch();
    let forge = forge_checked(&core.send(Command::Attach));
    assert_eq!(forge.name, "GitHub");
    match forge.state {
        ForgeSetupState::Missing { install_hint, link } => {
            assert_eq!(install_hint, "brew install gh");
            assert!(link.url.starts_with("https://"));
        }
        other => panic!("expected Missing, got {other:?}"),
    }
}

#[test]
fn a_signed_out_gh_is_a_setup_state_on_attach() {
    let mut core = Harness::launch();
    core.gh.install();
    core.gh.sign_out();
    let forge = forge_checked(&core.send(Command::Attach));
    assert_eq!(forge.state, ForgeSetupState::SignedOut { login_hint: "gh auth login".into() });
}

#[test]
fn a_signed_in_gh_is_ready_and_check_forge_detects_again() {
    let mut core = Harness::launch();
    let forge = forge_checked(&core.send(Command::Attach));
    assert!(matches!(forge.state, ForgeSetupState::Missing { .. }));

    core.gh.install();
    // Detection is cached: attaching again does not run gh.
    assert!(matches!(forge_checked(&core.send(Command::Attach)).state, ForgeSetupState::Missing { .. }));
    let forge = forge_checked(&core.send(Command::CheckForge));
    assert_eq!(forge.state, ForgeSetupState::Ready { version: "2.81.0".into() });
}

/// A repository whose `origin` is a local bare repository, with `main` pushed, a
/// repo-local identity, and a branch worktree `feat/login` one commit ahead of main.
struct Remote {
    repo: TempRepo,
    bare: tempfile::TempDir,
    wt: PathBuf,
    core: Harness,
    repo_id: String,
}

fn with_remote() -> Remote {
    let repo = TempRepo::new("acme");
    identity(&repo.path);
    let bare = tempfile::TempDir::new().unwrap();
    git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(&repo.path, &["remote", "add", "origin", bare.path().to_str().unwrap()]);
    git(&repo.path, &["push", "-q", "-u", "origin", "main"]);
    let wt = repo.add_worktree("feat/login");
    commit_file(&wt, "login.txt", "form\n", "Add the login form");
    let mut core = Harness::launch();
    core.gh.install();
    core.send(Command::Attach);
    let repo_id = added_repository(&core.add_repository(&repo.path)).id;
    Remote { repo, bare, wt, core, repo_id }
}

fn identity(dir: &Path) {
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
}

fn commit_file(dir: &Path, name: &str, text: &str, message: &str) {
    std::fs::write(dir.join(name), text).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn path(p: &Path) -> String {
    p.to_str().unwrap().into()
}

fn open_pr(core: &mut Harness, wt: &Path) -> Vec<Event> {
    core.send(Command::OpenPullRequest { worktree_path: path(wt) })
}

fn refusal(events: &[Event]) -> String {
    events
        .iter()
        .find_map(|e| match e {
            Event::CommandRefused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a refusal in {events:#?}"))
}

fn remote_has(bare: &Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .current_dir(bare)
        .status()
        .unwrap()
        .success()
}

fn open_at(number: u32) -> PullRequest {
    PullRequest {
        number,
        url: format!("https://github.com/acme/acme/pull/{number}"),
        state: PullRequestState::Open,
        draft: false,
        checks: ChecksState::None,
        check_runs: Vec::new(),
        review: ReviewState::None,
    }
}

#[test]
fn the_repository_names_its_remote() {
    let mut r = with_remote();
    let events = r.core.send(Command::Attach);
    let repo = events.iter().find_map(|e| match e {
        Event::RepositoryAdded { repository } => Some(repository.clone()),
        _ => None,
    });
    assert_eq!(repo.unwrap().remote.as_deref(), Some("origin"));
    let lone = TempRepo::new("lone");
    assert_eq!(added_repository(&r.core.add_repository(&lone.path)).remote, None);
}

#[test]
fn open_pull_request_pushes_the_branch_and_opens_one_against_base() {
    let mut r = with_remote();
    let events = open_pr(&mut r.core, &r.wt);

    assert!(remote_has(r.bare.path(), "feat/login"), "the branch is pushed");
    let creates: Vec<_> = r.core.gh.calls().into_iter().filter(|c| c.starts_with(&["pr".to_string(), "create".to_string()])).collect();
    assert_eq!(creates, vec![vec!["pr", "create", "--base", "main", "--head", "feat/login", "--fill"]]);
    assert_eq!(
        events,
        vec![
            Event::PullRequestOpened { worktree_path: path(&r.wt), pull_request: open_at(421) },
            Event::PullRequestChanged { worktree_path: path(&r.wt), pull_request: Some(open_at(421)), checked_at_ms: NOW_MS },
        ]
    );
    // The branch tracks the remote now.
    let upstream = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "feat/login@{upstream}"])
        .current_dir(&r.wt)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&upstream.stdout).trim(), "origin/feat/login");

    let again = open_pr(&mut r.core, &r.wt);
    assert_eq!(refusal(&again), "PR #421 is already open for feat/login");
}

#[test]
fn open_pull_request_is_refused_with_uncommitted_files() {
    let mut r = with_remote();
    std::fs::write(r.wt.join("a.txt"), "a").unwrap();
    std::fs::write(r.wt.join("b.txt"), "b").unwrap();
    let events = open_pr(&mut r.core, &r.wt);
    assert_eq!(refusal(&events), "feat/login has 2 uncommitted files; commit them before opening a pull request");
    assert!(!remote_has(r.bare.path(), "feat/login"));
    assert_eq!(r.core.gh.calls_of(["pr", "create"]), 0);
}

#[test]
fn open_pull_request_is_refused_when_behind_base() {
    let mut r = with_remote();
    commit_file(&r.repo.path, "news.txt", "news\n", "Base moves on");
    let events = open_pr(&mut r.core, &r.wt);
    assert_eq!(refusal(&events), "feat/login is 1 commit behind main; rebase before opening a pull request");
    assert!(!remote_has(r.bare.path(), "feat/login"));
}

#[test]
fn open_pull_request_is_refused_when_gh_is_missing_or_signed_out() {
    let mut r = with_remote();
    r.core.gh.sign_out();
    let events = open_pr(&mut r.core, &r.wt);
    assert_eq!(refusal(&events), "GitHub CLI (gh) is not signed in; run gh auth login, then try again");
    assert!(!remote_has(r.bare.path(), "feat/login"));

    let mut core = Harness::launch();
    let repo = TempRepo::new("acme");
    let bare = tempfile::TempDir::new().unwrap();
    git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(&repo.path, &["remote", "add", "origin", bare.path().to_str().unwrap()]);
    let wt = repo.add_worktree("feat/x");
    identity(&wt);
    commit_file(&wt, "x.txt", "x\n", "x");
    core.add_repository(&repo.path);
    let events = open_pr(&mut core, &wt);
    assert_eq!(refusal(&events), "GitHub CLI (gh) is not installed; install it (brew install gh) to open pull requests");
    assert!(!remote_has(bare.path(), "feat/x"));
}

#[test]
fn signing_in_after_a_refusal_is_noticed_on_the_next_try() {
    let mut r = with_remote();
    r.core.gh.sign_out();
    r.core.send(Command::CheckForge);
    r.core.gh.sign_in();
    let events = open_pr(&mut r.core, &r.wt);
    assert!(events.iter().any(
        |e| matches!(e, Event::ForgeChecked { forge } if matches!(forge.state, ForgeSetupState::Ready { .. }))
    ));
    assert!(events.iter().any(|e| matches!(e, Event::PullRequestOpened { .. })));
}

#[test]
fn open_pull_request_is_refused_with_nothing_to_ship_or_no_remote() {
    let mut r = with_remote();
    let fresh = r.repo.add_worktree("feat/empty");
    assert_eq!(refusal(&open_pr(&mut r.core, &fresh)), "feat/empty has no commits above main; commit the work first");
    assert_eq!(
        refusal(&open_pr(&mut r.core, &r.repo.path)),
        "main is the primary worktree; open pull requests from a branch worktree"
    );

    let lone = TempRepo::new("lone");
    let wt = lone.add_worktree("feat/y");
    r.core.add_repository(&lone.path);
    assert_eq!(refusal(&open_pr(&mut r.core, &wt)), "feat/y's repository has no remote; use Local merge instead");
}

fn changed(events: &[Event], wt: &Path) -> Vec<Option<PullRequest>> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::PullRequestChanged { worktree_path, pull_request, .. } if *worktree_path == path(wt) => {
                Some(pull_request.clone())
            }
            _ => None,
        })
        .collect()
}

fn merged(events: &[Event]) -> Vec<Event> {
    events.iter().filter(|e| matches!(e, Event::BranchMerged { .. })).cloned().collect()
}

fn removed(events: &[Event], wt: &Path) -> bool {
    events.iter().any(|e| matches!(e, Event::WorktreeRemoved { worktree_path, .. } if *worktree_path == path(wt)))
}

fn head(dir: &Path) -> String {
    let out = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(dir).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The pull requests without their individual checks (see the next test for those).
fn summed_up(prs: Vec<Option<PullRequest>>) -> Vec<Option<PullRequest>> {
    prs.into_iter().map(|p| p.map(|p| PullRequest { check_runs: Vec::new(), ..p })).collect()
}

#[test]
fn polling_reports_checks_and_review_as_they_change() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);

    r.core.gh.set_checks("feat/login", json!([{"__typename": "CheckRun", "status": "IN_PROGRESS", "conclusion": ""}]));
    let running = summed_up(changed(&r.core.poll_forge(), &r.wt));
    assert_eq!(running, vec![Some(PullRequest { checks: ChecksState::Running, ..open_at(421) })]);

    r.core.gh.set_checks(
        "feat/login",
        json!([
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "StatusContext", "state": "FAILURE"},
        ]),
    );
    let failing = summed_up(changed(&r.core.poll_forge(), &r.wt));
    assert_eq!(failing, vec![Some(PullRequest { checks: ChecksState::Failing, ..open_at(421) })]);

    r.core.gh.set_checks("feat/login", json!([{"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"}]));
    r.core.gh.set_review("feat/login", "APPROVED");
    let approved = summed_up(changed(&r.core.poll_forge(), &r.wt));
    assert_eq!(
        approved,
        vec![Some(PullRequest { checks: ChecksState::Passing, review: ReviewState::Approved, ..open_at(421) })]
    );
}

#[test]
fn polling_reports_each_check_with_its_name_status_and_link() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);

    r.core.gh.set_checks(
        "feat/login",
        json!([
            {"__typename": "CheckRun", "name": "build", "workflowName": "CI", "status": "COMPLETED",
             "conclusion": "SUCCESS", "detailsUrl": "https://github.com/acme/acme/runs/1"},
            {"__typename": "CheckRun", "name": "test", "status": "IN_PROGRESS", "conclusion": "",
             "detailsUrl": "https://github.com/acme/acme/runs/2"},
            {"__typename": "CheckRun", "name": "deploy", "status": "QUEUED", "conclusion": ""},
            {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "TIMED_OUT"},
            {"__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SKIPPED"},
            {"__typename": "StatusContext", "context": "ci/legacy", "state": "ERROR",
             "targetUrl": "https://ci.example.com/7"},
            {"__typename": "StatusContext", "context": "coverage", "state": "PENDING"},
        ]),
    );
    let polled = changed(&r.core.poll_forge(), &r.wt);
    let pr = polled.last().cloned().flatten().expect("a pull request");
    assert_eq!(pr.checks, ChecksState::Failing);
    let runs: Vec<(String, CheckStatus, Option<String>)> =
        pr.check_runs.into_iter().map(|c| (c.name, c.status, c.url)).collect();
    assert_eq!(
        runs,
        vec![
            ("build".into(), CheckStatus::Passing, Some("https://github.com/acme/acme/runs/1".into())),
            ("test".into(), CheckStatus::Running, Some("https://github.com/acme/acme/runs/2".into())),
            ("deploy".into(), CheckStatus::Pending, None),
            ("lint".into(), CheckStatus::Failing, None),
            ("docs".into(), CheckStatus::Skipped, None),
            ("ci/legacy".into(), CheckStatus::Failing, Some("https://ci.example.com/7".into())),
            ("coverage".into(), CheckStatus::Pending, None),
        ]
    );
}

/// The ticks (1-based) on which `pr list` ran, over `ticks` ticks.
fn polled_on(core: &mut Harness, ticks: u32) -> Vec<u32> {
    let mut on = Vec::new();
    for tick in 1..=ticks {
        let before = core.gh.calls_of(["pr", "list"]);
        core.poll_forge();
        if core.gh.calls_of(["pr", "list"]) > before {
            on.push(tick);
        }
    }
    on
}

/// The `checkedAtMs` of every `PullRequestChanged` for the worktree.
fn checked(events: &[Event], wt: &Path) -> Vec<u64> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::PullRequestChanged { worktree_path, checked_at_ms, .. } if *worktree_path == path(wt) => {
                Some(*checked_at_ms)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_pull_request_says_when_it_was_last_checked_even_when_nothing_changed() {
    let mut r = with_remote();
    assert_eq!(checked(&open_pr(&mut r.core, &r.wt), &r.wt), vec![NOW_MS]);
    assert_eq!(checked(&r.core.poll_forge(), &r.wt), vec![NOW_MS], "polled again, unchanged");

    let core = std::mem::replace(&mut r.core, Harness::launch()).relaunch_at(|| NOW_MS + 15_000);
    r.core = core;
    r.core.send(Command::Attach);
    assert_eq!(checked(&r.core.poll_forge(), &r.wt), vec![NOW_MS + 15_000]);
    let replayed = r.core.send(Command::Attach);
    assert_eq!(checked(&replayed, &r.wt), vec![NOW_MS + 15_000], "attach replays the last check");
}

#[test]
fn polling_backs_off_while_nothing_changes_and_speeds_up_on_a_change() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);
    // 1, then every 2, 4, 8 ticks, never further apart than 8.
    assert_eq!(polled_on(&mut r.core, 32), vec![1, 3, 7, 15, 23, 31]);

    r.core.gh.set_review("feat/login", "CHANGES_REQUESTED");
    // Next due on tick 39 (8 after 31), the 7th of these; it sees the change, so the next
    // poll is on the next tick, then they back off again.
    let on = polled_on(&mut r.core, 12);
    assert_eq!(on, vec![7, 8, 10]);
}

#[test]
fn a_hidden_window_polls_every_pull_request_at_the_slowest_cadence() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);
    let before = r.core.gh.calls_of(["pr", "list"]);
    r.core.send(Command::WindowVisibility { visible: false });
    assert_eq!(r.core.gh.calls_of(["pr", "list"]), before, "hiding polls nothing");
    // Due on the first tick, then every 8 ticks: never faster, even when something changes.
    assert_eq!(polled_on(&mut r.core, 24), vec![1, 9, 17]);
    r.core.gh.set_review("feat/login", "CHANGES_REQUESTED");
    assert_eq!(polled_on(&mut r.core, 16), vec![1, 9]);
}

#[test]
fn poll_forge_now_polls_at_once_and_resets_the_back_off() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);
    assert_eq!(polled_on(&mut r.core, 16), vec![1, 3, 7, 15], "backed off to every 8 ticks");
    let before = r.core.gh.calls_of(["pr", "list"]);
    r.core.send(Command::PollForgeNow);
    assert_eq!(r.core.gh.calls_of(["pr", "list"]), before + 1, "polled at once");
    // Backing off afresh from that poll: 2, 4, 8 ticks apart.
    assert_eq!(polled_on(&mut r.core, 16), vec![2, 6, 14]);
}

#[test]
fn becoming_visible_polls_at_once_and_backs_off_afresh() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);
    r.core.send(Command::WindowVisibility { visible: false });
    assert_eq!(polled_on(&mut r.core, 4), vec![1]);
    let before = r.core.gh.calls_of(["pr", "list"]);
    r.core.send(Command::WindowVisibility { visible: true });
    assert_eq!(r.core.gh.calls_of(["pr", "list"]), before + 1, "polled at once");
    assert_eq!(polled_on(&mut r.core, 8), vec![2, 6]);
}

#[test]
fn a_closed_pull_request_is_still_polled_now_and_then_and_a_reopen_is_seen() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);
    r.core.gh.set_state("feat/login", "CLOSED");
    let events = r.core.poll_forge();
    assert_eq!(changed(&events, &r.wt), vec![Some(PullRequest { state: PullRequestState::Closed, ..open_at(421) })]);
    assert!(merged(&events).is_empty());
    assert!(r.wt.exists(), "a closed pull request removes nothing");
    // Rarely: every CLOSED_TICKS ticks.
    let every = switchyard_core::forge::CLOSED_TICKS;
    assert_eq!(polled_on(&mut r.core, every * 2), vec![every, every * 2]);

    // Reopened in the browser: seen on the next of those polls, then polled as usual.
    r.core.gh.set_state("feat/login", "OPEN");
    let mut events = Vec::new();
    for _ in 0..every {
        events.extend(r.core.poll_forge());
    }
    assert_eq!(changed(&events, &r.wt), vec![Some(open_at(421))]);
    assert_eq!(polled_on(&mut r.core, 1), vec![1]);
}

#[test]
fn opening_another_pull_request_after_one_closed_polls_it_at_once() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);
    r.core.gh.set_state("feat/login", "CLOSED");
    r.core.poll_forge();
    let events = open_pr(&mut r.core, &r.wt);
    assert!(events.iter().any(|e| matches!(e, Event::PullRequestOpened { pull_request, .. } if pull_request.number == 422)));
    assert_eq!(polled_on(&mut r.core, 1), vec![1]);
}

#[test]
fn a_merge_in_the_browser_cleans_the_worktree_up_on_the_next_poll() {
    let mut r = with_remote();
    let session = harness::started_session(&r.core.start_session(&r.wt, "Add a login form."));
    r.core.agent_says(0, adapter_protocol::Event::Done { summary: None });
    open_pr(&mut r.core, &r.wt);
    r.core.poll_forge(); // tick 1: nothing new; next poll is two ticks away

    r.core.gh.set_state("feat/login", "MERGED");
    let mut events = r.core.poll_forge();
    assert!(!removed(&events, &r.wt), "not due on this tick");
    events.extend(r.core.poll_forge());

    assert!(removed(&events, &r.wt));
    assert!(!r.wt.exists());
    assert!(events.iter().any(|e| matches!(e, Event::SessionArchived { archived } if archived.session.id == session)));
    assert_eq!(
        merged(&events),
        vec![Event::BranchMerged {
            repository_id: r.repo_id.clone(),
            worktree_path: path(&r.wt),
            branch: "feat/login".into(),
            base: "main".into(),
            pull_request: Some(421),
            removed: true,
            kept: None,
        }]
    );
    assert_eq!(polled_on(&mut r.core, 20), Vec::<u32>::new(), "nothing left to poll");
}

#[test]
fn a_pull_request_opened_outside_switchyard_is_found_and_its_merge_cleans_up() {
    let mut r = with_remote();
    git(&r.wt, &["push", "-q", "origin", "feat/login"]);
    r.core.gh.set_prs("feat/login", json!([gh_pr(7, "OPEN", &head(&r.wt))]));
    let events = r.core.poll_forge();
    assert_eq!(changed(&events, &r.wt), vec![Some(open_at(7))]);

    r.core.gh.set_state("feat/login", "MERGED");
    let events = r.core.poll_forge();
    assert!(removed(&events, &r.wt));
}

#[test]
fn a_merged_pull_request_found_after_a_relaunch_still_cleans_up() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);
    r.core.gh.set_state("feat/login", "MERGED");
    let mut core = r.core.relaunch();
    core.send(Command::Attach);
    let events = core.poll_forge();
    assert!(removed(&events, &r.wt));
}

#[test]
fn an_old_merged_pull_request_for_a_reused_branch_name_is_ignored() {
    let mut r = with_remote();
    r.core.gh.set_prs("feat/login", json!([gh_pr(3, "MERGED", "0123456789abcdef0123456789abcdef01234567")]));
    let events = r.core.poll_forge();
    assert!(changed(&events, &r.wt).is_empty());
    assert!(r.wt.exists());
}

#[test]
fn a_merged_worktree_with_uncommitted_files_or_an_agent_at_work_is_kept() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);
    std::fs::write(r.wt.join("scratch.txt"), "notes").unwrap();
    r.core.gh.set_state("feat/login", "MERGED");
    let events = r.core.poll_forge();
    assert!(r.wt.exists());
    match &merged(&events)[..] {
        [Event::BranchMerged { removed: false, kept: Some(kept), .. }] => assert_eq!(
            kept,
            "feat/login has 1 uncommitted file; commit or discard it before removing the worktree"
        ),
        other => panic!("expected one kept BranchMerged, got {other:#?}"),
    }

    let other = r.repo.add_worktree("feat/busy");
    commit_file(&other, "busy.txt", "busy\n", "Busy");
    r.core.start_session(&other, "Keep going.");
    r.core.gh.set_prs("feat/busy", json!([gh_pr(9, "MERGED", &head(&other))]));
    let events = r.core.poll_forge();
    assert!(other.exists());
    assert!(merged(&events).iter().any(|e| matches!(e,
        Event::BranchMerged { kept: Some(k), .. } if k == "feat/busy has a Running session; stop it before removing the worktree")));
}

#[test]
fn a_merged_worktree_kept_for_its_files_is_cleaned_up_once_they_are_committed() {
    let mut r = with_remote();
    open_pr(&mut r.core, &r.wt);
    std::fs::write(r.wt.join("scratch.txt"), "notes").unwrap();
    r.core.gh.set_state("feat/login", "MERGED");
    let events = r.core.poll_forge();
    assert!(merged(&events).iter().any(|e| matches!(e, Event::BranchMerged { removed: false, .. })));
    let lists = r.core.gh.calls_of(["pr", "list"]);

    // Still dirty: kept, quietly, tick after tick.
    let events = r.core.poll_forge();
    assert!(merged(&events).is_empty());
    assert!(r.wt.exists());

    std::fs::remove_file(r.wt.join("scratch.txt")).unwrap();
    let events = r.core.poll_forge();
    assert!(!r.wt.exists(), "removed once clean");
    assert_eq!(
        merged(&events),
        vec![Event::BranchMerged {
            repository_id: r.repo_id.clone(),
            worktree_path: path(&r.wt),
            branch: "feat/login".into(),
            base: "main".into(),
            pull_request: Some(421),
            removed: true,
            kept: None,
        }]
    );
    assert_eq!(r.core.gh.calls_of(["pr", "list"]), lists, "a merged pull request is not asked about again");
}

#[test]
fn a_merged_worktree_kept_for_its_session_is_cleaned_up_when_the_session_stops() {
    let mut r = with_remote();
    let session = harness::started_session(&r.core.start_session(&r.wt, "Polish."));
    open_pr(&mut r.core, &r.wt);
    r.core.gh.set_state("feat/login", "MERGED");
    let events = r.core.poll_forge();
    assert!(merged(&events).iter().any(|e| matches!(e, Event::BranchMerged { removed: false, .. })));

    r.core.send(Command::StopSession { session_id: session });
    let events = r.core.poll_forge();
    assert!(removed(&events, &r.wt), "{events:#?}");
}

#[test]
fn nothing_is_polled_without_a_signed_in_gh_or_without_a_remote() {
    let mut r = with_remote();
    r.core.gh.sign_out();
    r.core.send(Command::CheckForge);
    assert_eq!(polled_on(&mut r.core, 3), Vec::<u32>::new());

    let mut core = Harness::launch();
    core.gh.install();
    let lone = TempRepo::new("lone");
    lone.add_worktree("feat/y");
    core.add_repository(&lone.path);
    assert_eq!(polled_on(&mut core, 3), Vec::<u32>::new());
}

#[test]
fn prune_merged_removes_every_free_clean_merged_worktree() {
    let mut r = with_remote();
    let done = r.repo.add_worktree("feat/done");
    commit_file(&done, "done.txt", "done\n", "Done");
    let dirty = r.repo.add_worktree("feat/dirty");
    commit_file(&dirty, "dirty.txt", "dirty\n", "Dirty");
    std::fs::write(dirty.join("wip.txt"), "wip").unwrap();
    let open = r.repo.add_worktree("feat/open");
    commit_file(&open, "open.txt", "open\n", "Open");
    // The poller already saw feat/login merged but could not clean it up (an agent was at
    // work); since then the agent stopped.
    r.core.gh.set_prs("feat/login", json!([gh_pr(1, "MERGED", &head(&r.wt))]));
    r.core.gh.set_prs("feat/done", json!([gh_pr(2, "MERGED", &head(&done))]));
    r.core.gh.set_prs("feat/dirty", json!([gh_pr(3, "MERGED", &head(&dirty))]));
    r.core.gh.set_prs("feat/open", json!([gh_pr(4, "OPEN", &head(&open))]));

    let events = r.core.send(Command::PruneMerged { repository_id: r.repo_id.clone() });

    assert!(removed(&events, &r.wt) && removed(&events, &done));
    assert!(dirty.exists() && open.exists());
    assert!(events.contains(&Event::MergedPruned {
        repository_id: r.repo_id.clone(),
        branches: vec!["feat/done".into(), "feat/login".into()],
    }));
}

#[test]
fn a_failed_create_says_the_branch_was_pushed() {
    let mut r = with_remote();
    r.core.gh.fail_create("GraphQL: No commits between main and feat/login");
    let events = open_pr(&mut r.core, &r.wt);
    assert_eq!(
        refusal(&events),
        "feat/login was pushed, but the pull request could not be opened: GraphQL: No commits between main and feat/login"
    );
}

/// A repository with no remote and a branch worktree one commit ahead of main.
fn without_remote() -> (TempRepo, PathBuf, Harness, String) {
    let repo = TempRepo::new("lone");
    identity(&repo.path);
    let wt = repo.add_worktree("feat/local");
    commit_file(&wt, "local.txt", "local\n", "Add local work");
    let mut core = Harness::launch();
    let id = added_repository(&core.add_repository(&repo.path)).id;
    (repo, wt, core, id)
}

fn local_merge(core: &mut Harness, wt: &Path) -> Vec<Event> {
    core.send(Command::LocalMerge { worktree_path: path(wt) })
}

#[test]
fn local_merge_merges_into_base_in_the_primary_worktree_and_cleans_up() {
    let (repo, wt, mut core, repo_id) = without_remote();
    let tip = head(&wt);
    let events = local_merge(&mut core, &wt);

    assert!(removed(&events, &wt));
    assert!(!wt.exists());
    assert!(repo.path.join("local.txt").exists(), "base now has the branch's work");
    assert!(
        std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", &tip, "main"])
            .current_dir(&repo.path)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        merged(&events),
        vec![Event::BranchMerged {
            repository_id: repo_id,
            worktree_path: path(&wt),
            branch: "feat/local".into(),
            base: "main".into(),
            pull_request: None,
            removed: true,
            kept: None,
        }]
    );
}

#[test]
fn local_merge_is_refused_whenever_a_remote_exists() {
    let mut r = with_remote();
    let events = local_merge(&mut r.core, &r.wt);
    assert_eq!(
        refusal(&events),
        "This repository has a remote: work leaves through a pull request, never a local merge"
    );
    assert!(r.wt.exists());
    assert!(!r.repo.path.join("login.txt").exists());
}

#[test]
fn local_merge_is_refused_with_uncommitted_files_a_busy_primary_or_conflicts() {
    let (repo, wt, mut core, _) = without_remote();
    std::fs::write(wt.join("wip.txt"), "wip").unwrap();
    assert_eq!(refusal(&local_merge(&mut core, &wt)), "feat/local has 1 uncommitted file; commit it before merging");
    std::fs::remove_file(wt.join("wip.txt")).unwrap();

    std::fs::write(repo.path.join("README"), "edited\n").unwrap();
    assert_eq!(
        refusal(&local_merge(&mut core, &wt)),
        "The primary worktree has 1 uncommitted file; commit it before merging into main"
    );
    git(&repo.path, &["commit", "-q", "-am", "Edit README on main"]);

    commit_file(&wt, "README", "branch edit\n", "Edit README on the branch");
    assert_eq!(
        refusal(&local_merge(&mut core, &wt)),
        "Merging feat/local into main would conflict in README; rebase feat/local first"
    );
    assert!(wt.exists());
}

/// M2 gate: a session is started and its pull request opened without a terminal; the
/// worktree is gone (shells closed, session archived) after the remote reports the merge.
#[test]
fn gate_a_session_ships_through_a_pull_request_and_is_cleaned_up_when_merged() {
    let mut r = with_remote();
    r.core.use_shell("/bin/sh");
    let events = r.core.send(Command::StartSessionOnNewWorktree {
        repository_id: r.repo_id.clone(),
        branch: "feat/signup".into(),
        agent: switchyard_schema::AgentKind::ClaudeCode,
        permission_mode: switchyard_schema::PermissionMode::AutoEdit,
        prompt: "Add a signup page.".into(),
    });
    let session = harness::started_session(&events);
    let wt = events
        .iter()
        .find_map(|e| match e {
            Event::WorktreeAdded { worktree, .. } => Some(PathBuf::from(&worktree.path)),
            _ => None,
        })
        .unwrap();
    identity(&wt);
    // The agent writes a file and finishes; the user reviews and commits it in Switchyard.
    std::fs::write(wt.join("signup.txt"), "signup\n").unwrap();
    r.core.agent_says(0, adapter_protocol::Event::Done { summary: None });
    r.core.send(Command::ReadChanges { worktree_path: path(&wt) });
    r.core.send(Command::DecideFile {
        worktree_path: path(&wt),
        path: "signup.txt".into(),
        decision: Some(switchyard_schema::Decision::Accept),
    });
    let events = r.core.send(Command::CommitAccepted { worktree_path: path(&wt), message: "Add signup".into() });
    assert!(events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:#?}");
    r.core.dispatch(Command::OpenShell { worktree_path: path(&wt), cols: 80, rows: 24 });
    r.core.wait_for(|seen| seen.iter().any(|e| matches!(e, Event::ShellOpened { .. })));

    let events = open_pr(&mut r.core, &wt);
    assert!(events.iter().any(|e| matches!(e, Event::PullRequestOpened { .. })), "{events:#?}");
    assert!(remote_has(r.bare.path(), "feat/signup"));

    r.core.gh.set_state("feat/signup", "MERGED");
    let events = r.core.poll_forge();
    assert!(removed(&events, &wt));
    assert!(!wt.exists());
    assert!(events.iter().any(|e| matches!(e, Event::ShellClosed { .. })));
    assert!(events.iter().any(|e| matches!(e, Event::SessionArchived { archived } if archived.session.id == session)));
    assert!(merged(&events).iter().any(|e| matches!(e, Event::BranchMerged { removed: true, .. })));
}
