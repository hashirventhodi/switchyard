//! Behind, rebase and conflict detection through seam 1, against real temporary git
//! repositories: ahead/behind and conflicting files read from git, a dry-run merge that
//! never touches the working tree or the index, rebase that completes or stops on real
//! conflicts for resolving (and continues when asked again),
//! and the branch's history above its base.
mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, git, worktree_snapshot, Harness, TempRepo};
use adapter_protocol::{Command as AgentCommand, Event as AgentEvent};
use switchyard_schema::{Command, ConflictKind, ConflictSide, Entry, Event, SessionStatus, WorktreeFacts};

/// A repository with an identity for the commits a rebase rewrites (CI has no global one),
/// added to a fresh core, plus a worktree on `feat/login` made through the core.
fn setup() -> (TempRepo, Harness, PathBuf) {
    let repo = TempRepo::new("acme");
    git(&repo.path, &["config", "user.name", "Test"]);
    git(&repo.path, &["config", "user.email", "test@example.com"]);
    let mut core = Harness::launch();
    let repository = added_repository(&core.add_repository(&repo.path));
    let events = core.send(Command::CreateWorktree { repository_id: repository.id, branch: "feat/login".into() });
    let path = events
        .iter()
        .find_map(|e| match e {
            Event::WorktreeAdded { worktree, .. } => Some(PathBuf::from(&worktree.path)),
            _ => None,
        })
        .expect("worktree created");
    (repo, core, path)
}

fn commit(dir: &Path, file: &str, text: &str, message: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn facts_of(events: &[Event], path: &Path) -> WorktreeFacts {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::WorktreeFactsChanged { worktree_path, facts } if Path::new(worktree_path) == path => Some(facts.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no facts for {path:?} in {events:#?}"))
}

fn refresh(core: &mut Harness) -> Vec<Event> {
    core.send(Command::RefreshWorktrees)
}

/// The index file's bytes: changes if anything is staged, unstaged or the index rewritten.
fn index_bytes(worktree: &Path) -> Vec<u8> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", "index"])
        .current_dir(worktree)
        .output()
        .unwrap();
    let index = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let index = if index.is_absolute() { index } else { worktree.join(index) };
    std::fs::read(index).unwrap()
}

#[test]
fn a_dry_run_merge_names_the_files_that_would_conflict_without_touching_the_tree_or_index() {
    let (repo, mut core, path) = setup();
    commit(&path, "README", "hello from the branch\n", "branch edit");
    commit(&repo.path, "README", "hello from main\n", "main edit");
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");
    let tree_before = worktree_snapshot(&path);
    let index_before = index_bytes(&path);

    let facts = facts_of(&refresh(&mut core), &path);

    assert_eq!(facts, WorktreeFacts { ahead: 1, behind: 2, dirty: 0, conflicts: vec!["README".into()], changed_files: 1, commits: 1 });
    assert_eq!(worktree_snapshot(&path), tree_before);
    assert_eq!(index_bytes(&path), index_before);
}

#[test]
fn a_branch_behind_with_disjoint_changes_has_no_conflicts() {
    let (repo, mut core, path) = setup();
    commit(&path, "login.rs", "fn login() {}\n", "add login");
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");

    let facts = facts_of(&refresh(&mut core), &path);

    assert_eq!(facts, WorktreeFacts { ahead: 1, behind: 1, dirty: 0, conflicts: vec![], changed_files: 0, commits: 1 });
}

fn rebase(core: &mut Harness, path: &Path) -> Vec<Event> {
    core.send(Command::RebaseWorktree { worktree_path: path.to_string_lossy().into_owned() })
}

fn refused(events: &[Event]) -> String {
    match events {
        [Event::CommandRefused { command, reason }] if command == "rebaseWorktree" => reason.clone(),
        other => panic!("expected one rebaseWorktree refusal, got {other:#?}"),
    }
}

fn head(dir: &Path) -> String {
    let out = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(dir).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn rebase_in_progress(worktree: &Path) -> bool {
    ["rebase-merge", "rebase-apply"].iter().any(|d| {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "--git-path", d])
            .current_dir(worktree)
            .output()
            .unwrap();
        let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        (if p.is_absolute() { p } else { worktree.join(p) }).exists()
    })
}

#[test]
fn a_rebase_without_conflicts_puts_the_branch_on_base_and_clears_behind() {
    let (repo, mut core, path) = setup();
    commit(&path, "login.rs", "fn login() {}\n", "add login");
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");

    let events = rebase(&mut core, &path);

    assert!(
        events.iter().any(|e| matches!(e, Event::WorktreeRebased { worktree_path, base }
            if Path::new(worktree_path) == path && base == "main")),
        "{events:#?}"
    );
    assert_eq!(facts_of(&events, &path), WorktreeFacts { ahead: 1, behind: 0, dirty: 0, conflicts: vec![], changed_files: 0, commits: 1 });
    assert!(path.join("NOTES").is_file(), "base's commit is under the branch now");
    assert!(path.join("login.rs").is_file(), "the branch's commit is kept");
}

#[test]
fn a_rebase_that_hits_conflicts_stops_with_them_in_the_tree_and_rebase_again_continues() {
    let (repo, mut core, path) = setup();
    commit(&path, "README", "hello from the branch\n", "branch edit");
    commit(&repo.path, "README", "hello from main\n", "main edit");

    let events = rebase(&mut core, &path);

    assert!(
        events.iter().any(|e| matches!(e, Event::RebaseStopped { base, conflicts, .. }
            if base == "main" && *conflicts == vec!["README".to_string()])),
        "{events:#?}"
    );
    assert!(!events.iter().any(|e| matches!(e, Event::CommandRefused { .. })), "not a refusal: {events:#?}");
    assert!(rebase_in_progress(&path), "left stopped, not aborted");
    // Real conflicts now: the Changes tab gets them with their hunks.
    let files = events
        .iter()
        .find_map(|e| match e {
            Event::ConflictsRead { files, .. } => Some(files.clone()),
            _ => None,
        })
        .expect("conflicts read");
    assert_eq!(files.iter().map(|f| (f.path.as_str(), f.kind)).collect::<Vec<_>>(), vec![("README", ConflictKind::Unmerged)]);

    // Rebase again before resolving: refused, naming the file.
    assert!(refused(&rebase(&mut core, &path)).contains("README"));

    // Keep ours, then Rebase again continues and finishes it.
    let hunk = files[0].hunks[0].id.clone();
    core.send(Command::ResolveConflict {
        worktree_path: path.to_string_lossy().into_owned(),
        path: "README".into(),
        hunk_id: hunk,
        side: ConflictSide::Ours,
    });
    let events = rebase(&mut core, &path);
    assert!(events.iter().any(|e| matches!(e, Event::WorktreeRebased { .. })), "{events:#?}");
    assert!(!rebase_in_progress(&path));
    assert_eq!(std::fs::read_to_string(path.join("README")).unwrap(), "hello from the branch\n");
}

#[test]
fn a_worktree_with_uncommitted_files_is_not_rebased() {
    let (repo, mut core, path) = setup();
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");
    std::fs::write(path.join("draft.rs"), "wip\n").unwrap();
    let head_before = head(&path);

    let reason = refused(&rebase(&mut core, &path));

    assert!(reason.contains("1 uncommitted file"), "{reason}");
    assert_eq!(head(&path), head_before);
}

#[test]
fn the_primary_worktree_is_not_rebased() {
    let (repo, mut core, _path) = setup();

    let reason = refused(&rebase(&mut core, &repo.path));

    assert!(reason.contains("primary"), "{reason}");
}

#[test]
fn a_worktree_whose_agent_is_running_is_not_rebased_behind_its_back() {
    let (repo, mut core, path) = setup();
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");
    core.start_session(&path, "Add a login form.");
    let head_before = head(&path);

    let reason = refused(&rebase(&mut core, &path));

    assert!(reason.contains("Running"), "{reason}");
    assert_eq!(head(&path), head_before);
}

#[test]
fn a_worktree_whose_agent_waits_on_a_permission_request_is_not_rebased_mid_turn() {
    let (repo, mut core, path) = setup();
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");
    core.start_session(&path, "Add a login form.");
    core.agent_says(
        0,
        AgentEvent::PermissionRequest {
            request_id: "req-1".into(),
            request: adapter_protocol::RequestDetail::Command { command: "pnpm test".into(), cwd: None },
            reason: None,
        },
    );
    let head_before = head(&path);

    let reason = refused(&rebase(&mut core, &path));

    assert!(reason.contains("Needs you"), "{reason}");
    assert_eq!(head(&path), head_before);
}

#[test]
fn a_worktree_whose_agent_awaits_the_next_prompt_is_not_rebased() {
    let (repo, mut core, path) = setup();
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");
    core.start_session(&path, "Add a login form.");
    core.agent_says(0, AgentEvent::TurnComplete);
    let head_before = head(&path);

    let reason = refused(&rebase(&mut core, &path));

    assert!(reason.contains("Needs you") && reason.contains("Paused, Finished or Error"), "{reason}");
    assert_eq!(head(&path), head_before);
}

#[test]
fn a_paused_finished_or_failed_session_does_not_stop_a_rebase() {
    let (repo, mut core, path) = setup();
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");
    let id = harness::started_session(&core.start_session(&path, "Add a login form."));
    core.send(Command::StopSession { session_id: id });
    let events = rebase(&mut core, &path);
    assert!(events.iter().any(|e| matches!(e, Event::WorktreeRebased { .. })), "{events:#?}");
}

#[test]
fn asking_the_agent_to_rebase_sends_one_message_and_the_session_goes_running() {
    let (repo, mut core, path) = setup();
    commit(&path, "README", "hello from the branch\n", "branch edit");
    commit(&repo.path, "README", "hello from main\n", "main edit");
    let id = harness::started_session(&core.start_session(&path, "Add a login form."));
    core.agent_says(0, AgentEvent::TurnComplete);
    let sent_before = core.agents.commands(0).len();

    let events = core.send(Command::AskAgentToRebase { session_id: id.clone() });

    let sent = &core.agents.commands(0)[sent_before..];
    let [AgentCommand::SendText { text }] = sent else { panic!("expected one message, got {sent:?}") };
    assert!(text.contains("git rebase main") && text.contains("README"), "{text}");
    let users: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::TranscriptEntry { entry: Entry::User { .. }, .. }))
        .collect();
    assert_eq!(users.len(), 1, "{events:#?}");
    assert!(events.contains(&Event::SessionStatusChanged { session_id: id, status: SessionStatus::Running }));
}

#[test]
fn asking_a_running_agent_to_rebase_is_refused() {
    let (_repo, mut core, path) = setup();
    let id = harness::started_session(&core.start_session(&path, "Add a login form."));

    let events = core.send(Command::AskAgentToRebase { session_id: id });

    assert!(matches!(events.as_slice(), [Event::CommandRefused { .. }]), "{events:#?}");
    assert_eq!(core.agents.commands(0).len(), 1, "only the first prompt was sent");
}

#[test]
fn ahead_and_behind_are_reread_after_every_agent_turn() {
    let (repo, mut core, path) = setup();
    core.start_session(&path, "Add a login form.");
    commit(&path, "login.rs", "fn login() {}\n", "add login");
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");

    let events = core.agent_says(0, AgentEvent::TurnComplete);

    assert_eq!(facts_of(&events, &path), WorktreeFacts { ahead: 1, behind: 1, dirty: 0, conflicts: vec![], changed_files: 0, commits: 1 });
}

fn history(core: &mut Harness, path: &Path) -> (String, Vec<switchyard_schema::Commit>) {
    let events = core.send(Command::ReadHistory { worktree_path: path.to_string_lossy().into_owned() });
    match events.as_slice() {
        [Event::WorktreeHistory { worktree_path, base, commits }] if Path::new(worktree_path) == path => {
            (base.clone(), commits.clone())
        }
        other => panic!("expected the history of {path:?}, got {other:#?}"),
    }
}

#[test]
fn history_lists_the_branch_commits_above_base_newest_first() {
    let (repo, mut core, path) = setup();
    commit(&repo.path, "NOTES", "unrelated\n", "main notes");
    commit(&path, "login.rs", "fn login() {}\n", "Add login");
    commit(&path, "logout.rs", "fn logout() {}\n", "Add logout\n\nWith a body.");

    let (base, commits) = history(&mut core, &path);

    assert_eq!(base, "main");
    let subjects: Vec<_> = commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["Add logout", "Add login"]);
    assert_eq!(commits[0].hash, head(&path));
    assert!(commits[0].hash.starts_with(&commits[0].short) && commits[0].short.len() < commits[0].hash.len());
    assert_eq!(commits[0].author, "Test");
    assert!(commits[0].at_ms > 0);
}

#[test]
fn a_branch_with_no_commits_of_its_own_has_an_empty_history() {
    let (_repo, mut core, path) = setup();

    let (base, commits) = history(&mut core, &path);

    assert_eq!(base, "main");
    assert!(commits.is_empty());
}
