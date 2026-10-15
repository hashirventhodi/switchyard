//! Inline conflict resolve through seam 1: conflict hunks with both sides, Keep ours / Keep
//! base written as plain file writes, and the re-check that follows. Real temporary git
//! repositories throughout.
mod harness;

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use harness::{git, Harness, TempRepo};
use switchyard_schema::{Command, ConflictFile, ConflictKind, ConflictSide, Decision, Event};

/// A repository with a committer identity, README (five lines) and lib.txt, and a worktree
/// on `feat/x`, added to a core.
fn setup() -> (TempRepo, PathBuf, Harness) {
    let repo = TempRepo::new("acme");
    git(&repo.path, &["config", "user.name", "Test"]);
    git(&repo.path, &["config", "user.email", "test@example.com"]);
    std::fs::write(repo.path.join("README"), "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n").unwrap();
    std::fs::write(repo.path.join("lib.txt"), "lib\n").unwrap();
    git(&repo.path, &["add", "."]);
    git(&repo.path, &["commit", "-q", "-m", "seed"]);
    let wt = repo.add_worktree("feat/x");
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    (repo, wt, core)
}

/// The branch and base both rewrite lines two and eight of README, differently.
fn diverge(repo: &TempRepo, wt: &Path) {
    std::fs::write(wt.join("README"), "one\nTWO ours\nthree\nfour\nfive\nsix\nseven\nEIGHT ours\nnine\n").unwrap();
    git(wt, &["commit", "-qam", "ours"]);
    std::fs::write(repo.path.join("README"), "one\nTWO base\nthree\nfour\nfive\nsix\nseven\nEIGHT base\nnine\n").unwrap();
    git(&repo.path, &["commit", "-qam", "base"]);
}

/// Run git expecting it to stop on a conflict.
fn stops(dir: &Path, args: &[&str]) {
    let out = Process::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(!out.status.success(), "git {args:?} should stop on a conflict");
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn conflicts(events: &[Event]) -> Vec<ConflictFile> {
    events
        .iter()
        .find_map(|e| match e {
            Event::ConflictsRead { files, .. } => Some(files.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ConflictsRead in {events:#?}"))
}

fn read(core: &mut Harness, wt: &Path) -> Vec<Event> {
    core.send(Command::ReadChanges { worktree_path: path_str(wt) })
}

fn resolve(core: &mut Harness, wt: &Path, path: &str, hunk_id: &str, side: ConflictSide) -> Vec<Event> {
    core.send(Command::ResolveConflict {
        worktree_path: path_str(wt),
        path: path.into(),
        hunk_id: hunk_id.into(),
        side,
    })
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

fn has_markers(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("<<<<<<<") || l.starts_with("=======") || l.starts_with(">>>>>>>"))
}

fn unmerged(wt: &Path) -> String {
    let out = Process::new("git").args(["diff", "--name-only", "--diff-filter=U"]).current_dir(wt).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(core: &mut Harness, wt: &Path) -> Vec<Event> {
    core.send(Command::CommitAccepted { worktree_path: path_str(wt), message: "Update lib".into() })
}

#[test]
fn a_rebase_stopped_on_a_conflict_lists_its_hunks_with_the_branch_and_base_sides() {
    let (repo, wt, mut core) = setup();
    diverge(&repo, &wt);
    stops(&wt, &["rebase", "main"]);

    let files = conflicts(&read(&mut core, &wt));

    assert_eq!(files.len(), 1);
    let readme = &files[0];
    assert_eq!(readme.path, "README");
    assert_eq!(readme.kind, ConflictKind::Unmerged);
    assert_eq!(readme.hunks.len(), 2);
    // During a rebase git writes base first; the hunk names the branch's side "ours".
    assert_eq!(readme.hunks[0].ours, vec!["TWO ours"]);
    assert_eq!(readme.hunks[0].base, vec!["TWO base"]);
    assert_eq!(readme.hunks[0].before, vec!["one"]);
    assert_eq!(readme.hunks[0].after, vec!["three", "four", "five"]);
    assert_eq!(readme.hunks[1].ours, vec!["EIGHT ours"]);
    assert_eq!(readme.hunks[1].base, vec!["EIGHT base"]);
    assert_ne!(readme.hunks[0].id, readme.hunks[1].id);
}

#[test]
fn a_merge_stopped_on_a_conflict_names_the_branch_side_ours_too() {
    let (repo, wt, mut core) = setup();
    diverge(&repo, &wt);
    stops(&wt, &["merge", "-q", "main"]);

    let files = conflicts(&read(&mut core, &wt));

    assert_eq!(files[0].hunks[0].ours, vec!["TWO ours"]);
    assert_eq!(files[0].hunks[0].base, vec!["TWO base"]);
}

#[test]
fn keep_ours_and_keep_base_write_the_side_and_leave_no_conflict_markers() {
    let (repo, wt, mut core) = setup();
    diverge(&repo, &wt);
    stops(&wt, &["rebase", "main"]);
    let hunks = conflicts(&read(&mut core, &wt)).remove(0).hunks;

    let events = resolve(&mut core, &wt, "README", &hunks[0].id, ConflictSide::Base);
    assert!(events.iter().any(|e| matches!(e,
        Event::ConflictResolved { path, side: ConflictSide::Base, remaining: 1, .. } if path == "README")));
    // Re-checked at once: the file still has its other conflict.
    let left = conflicts(&events);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].hunks.len(), 1);
    assert_eq!(left[0].hunks[0].id, hunks[1].id);

    let events = resolve(&mut core, &wt, "README", &hunks[1].id, ConflictSide::Ours);
    assert!(events.iter().any(|e| matches!(e, Event::ConflictResolved { remaining: 0, .. })));
    assert_eq!(conflicts(&events), vec![]);

    let text = std::fs::read_to_string(wt.join("README")).unwrap();
    assert!(!has_markers(&text), "{text}");
    assert_eq!(text, "one\nTWO base\nthree\nfour\nfive\nsix\nseven\nEIGHT ours\nnine\n");
    // Git agrees the file is resolved (as `git add` marks it).
    assert_eq!(unmerged(&wt), "");
}

#[test]
fn commit_stays_refused_while_any_conflict_remains_and_works_once_none_do() {
    let (repo, wt, mut core) = setup();
    diverge(&repo, &wt);
    stops(&wt, &["rebase", "main"]);
    std::fs::write(wt.join("lib.txt"), "lib, edited\n").unwrap();
    let hunks = conflicts(&read(&mut core, &wt)).remove(0).hunks;
    core.send(Command::DecideFile { worktree_path: path_str(&wt), path: "lib.txt".into(), decision: Some(Decision::Accept) });

    resolve(&mut core, &wt, "README", &hunks[0].id, ConflictSide::Ours);
    assert_eq!(refusal(&commit(&mut core, &wt)), "Resolve the conflict in README before committing");

    resolve(&mut core, &wt, "README", &hunks[1].id, ConflictSide::Ours);
    let events = commit(&mut core, &wt);
    assert!(events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:#?}");
}

#[test]
fn a_hunk_that_is_gone_or_a_file_without_conflicts_is_refused() {
    let (repo, wt, mut core) = setup();
    diverge(&repo, &wt);
    stops(&wt, &["rebase", "main"]);
    let hunks = conflicts(&read(&mut core, &wt)).remove(0).hunks;
    resolve(&mut core, &wt, "README", &hunks[0].id, ConflictSide::Ours);

    assert_eq!(
        refusal(&resolve(&mut core, &wt, "README", &hunks[0].id, ConflictSide::Base)),
        "That conflict in README has changed; read the changes again"
    );
    assert_eq!(
        refusal(&resolve(&mut core, &wt, "lib.txt", &hunks[1].id, ConflictSide::Base)),
        "lib.txt has no conflict to resolve"
    );
}

#[test]
fn resolving_is_refused_while_the_agent_is_running() {
    let (repo, wt, mut core) = setup();
    diverge(&repo, &wt);
    stops(&wt, &["rebase", "main"]);
    let hunks = conflicts(&read(&mut core, &wt)).remove(0).hunks;
    core.start_session(&wt, "Rebase on main");

    assert_eq!(
        refusal(&resolve(&mut core, &wt, "README", &hunks[0].id, ConflictSide::Ours)),
        "This worktree has a Running session; stop it or ask the agent to resolve the conflict"
    );
    assert!(has_markers(&std::fs::read_to_string(wt.join("README")).unwrap()));
}

#[test]
fn a_conflict_with_base_shows_its_hunks_from_a_dry_run_merge_and_leaves_the_tree_alone() {
    let (repo, wt, mut core) = setup();
    diverge(&repo, &wt);
    let before = std::fs::read_to_string(wt.join("README")).unwrap();

    let files = conflicts(&read(&mut core, &wt));

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, ConflictKind::Base);
    assert_eq!(files[0].hunks[0].ours, vec!["TWO ours"]);
    assert_eq!(files[0].hunks[0].base, vec!["TWO base"]);
    assert_eq!(
        refusal(&resolve(&mut core, &wt, "README", &files[0].hunks[0].id, ConflictSide::Base)),
        "README conflicts with main only in a merge of main that has not happened; rebase, or ask the agent to rebase"
    );
    assert_eq!(std::fs::read_to_string(wt.join("README")).unwrap(), before);
}
