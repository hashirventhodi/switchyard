//! The Forge through seam 1: `gh` detection, Open pull request, polling, Merged cleanup,
//! Prune merged and Local merge. Repositories are real temporary git repositories with a
//! local bare remote; `gh` is a fake script (see `harness/gh.rs`). Nothing touches GitHub.

mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, gh_pr, git, Harness, TempRepo};
use serde_json::json;
use switchyard_schema::{
    ChecksState, Command, Event, ForgeSetup, ForgeSetupState, PullRequest, PullRequestState, ReviewState,
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
            Event::PullRequestChanged { worktree_path: path(&r.wt), pull_request: Some(open_at(421)) },
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
