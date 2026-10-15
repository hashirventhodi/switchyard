//! The Changes tab through seam 1: read a worktree's changes against the index, decide
//! files, and commit accepted files whole. Real temporary git repositories throughout.
mod harness;

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use harness::{git, Harness, TempRepo};
use switchyard_schema::{ChangedFile, Command, Decision, Event, FileStatus, LineKind};

/// A repository with a committer identity and a worktree on `feat/x`, added to a core.
fn setup() -> (TempRepo, PathBuf, Harness) {
    let repo = TempRepo::new("acme");
    git(&repo.path, &["config", "user.name", "Test"]);
    git(&repo.path, &["config", "user.email", "test@example.com"]);
    std::fs::write(repo.path.join("lib.txt"), numbered(1..=40)).unwrap();
    std::fs::write(repo.path.join("old.txt"), "going away\n").unwrap();
    git(&repo.path, &["add", "."]);
    git(&repo.path, &["commit", "-q", "-m", "seed"]);
    let wt = repo.add_worktree("feat/x");
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    (repo, wt, core)
}

fn numbered(range: std::ops::RangeInclusive<u32>) -> String {
    range.map(|n| format!("line {n}\n")).collect()
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn read(core: &mut Harness, wt: &Path) -> Vec<ChangedFile> {
    let events = core.send(Command::ReadChanges { worktree_path: path_str(wt) });
    events
        .into_iter()
        .find_map(|e| match e {
            Event::ChangesRead { files, .. } => Some(files),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ChangesRead"))
}

fn file<'a>(files: &'a [ChangedFile], path: &str) -> &'a ChangedFile {
    files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path} not in {files:#?}"))
}

#[test]
fn changes_list_modified_added_and_deleted_files_with_hunks_and_counts() {
    let (_repo, wt, mut core) = setup();
    let mut text = numbered(1..=40).replace("line 5\n", "line five\n");
    text.push_str("line 41\n");
    std::fs::write(wt.join("lib.txt"), text).unwrap();
    std::fs::write(wt.join("new.txt"), "fresh\nfile\n").unwrap();
    std::fs::remove_file(wt.join("old.txt")).unwrap();

    let files = read(&mut core, &wt);

    let lib = file(&files, "lib.txt");
    assert_eq!(lib.status, FileStatus::Modified);
    assert_eq!((lib.added, lib.removed), (2, 1));
    assert_eq!(lib.hunks.len(), 2, "{lib:#?}");
    let first = &lib.hunks[0];
    assert!(first.header.starts_with("@@ -2,7 +2,7 @@"), "{}", first.header);
    let removed = first.lines.iter().find(|l| l.kind == LineKind::Removed).unwrap();
    assert_eq!((removed.text.as_str(), removed.old, removed.new), ("line 5", Some(5), None));
    let added = first.lines.iter().find(|l| l.kind == LineKind::Added).unwrap();
    assert_eq!((added.text.as_str(), added.old, added.new), ("line five", None, Some(5)));

    let new = file(&files, "new.txt");
    assert_eq!((new.status, new.added, new.removed), (FileStatus::Added, 2, 0));
    let old = file(&files, "old.txt");
    assert_eq!((old.status, old.added, old.removed), (FileStatus::Deleted, 0, 1));
    assert_eq!(files.len(), 3);
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Process::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn reading_changes_never_writes_the_index_or_the_tree() {
    let (_repo, wt, mut core) = setup();
    std::fs::write(wt.join("lib.txt"), "rewritten\n").unwrap();
    std::fs::write(wt.join("untracked.txt"), "u\n").unwrap();
    let before = harness::worktree_snapshot(&wt);
    let staged_before = git_out(&wt, &["diff", "--cached", "--name-only"]);

    read(&mut core, &wt);

    assert_eq!(harness::worktree_snapshot(&wt), before);
    assert_eq!(git_out(&wt, &["diff", "--cached", "--name-only"]), staged_before);
    assert!(git_out(&wt, &["status", "--porcelain"]).contains("?? untracked.txt"), "still untracked");
}

#[test]
fn a_hunk_keeps_its_id_when_lines_shift_above_it() {
    let (_repo, wt, mut core) = setup();
    let edit_late = numbered(1..=40).replace("line 30\n", "line thirty\n");
    std::fs::write(wt.join("lib.txt"), &edit_late).unwrap();
    let before = read(&mut core, &wt);
    let late_id = file(&before, "lib.txt").hunks[0].id.clone();

    // Two new lines near the top shift the later hunk's working-tree side.
    std::fs::write(wt.join("lib.txt"), edit_late.replace("line 3\n", "line 3\nextra a\nextra b\n")).unwrap();
    let after = read(&mut core, &wt);

    let lib = file(&after, "lib.txt");
    assert_eq!(lib.hunks.len(), 2);
    assert_ne!(lib.hunks[0].id, late_id);
    assert_eq!(lib.hunks[1].id, late_id, "the unchanged hunk keeps its id");
}

#[test]
fn renaming_a_file_keeps_ids_of_its_unchanged_hunks() {
    let (_repo, wt, mut core) = setup();
    let edited = numbered(1..=40).replace("line 5\n", "line five\n").replace("line 30\n", "line thirty\n");
    std::fs::write(wt.join("lib.txt"), &edited).unwrap();
    let before = read(&mut core, &wt);
    let ids: Vec<String> = file(&before, "lib.txt").hunks.iter().map(|h| h.id.clone()).collect();

    std::fs::remove_file(wt.join("lib.txt")).unwrap();
    std::fs::write(wt.join("core.txt"), edited.replace("line thirty\n", "line 30!\n")).unwrap();
    let after = read(&mut core, &wt);

    assert_eq!(after.len(), 1, "{after:#?}");
    let moved = file(&after, "core.txt");
    assert_eq!(moved.status, FileStatus::Renamed);
    assert_eq!(moved.old_path.as_deref(), Some("lib.txt"));
    assert_eq!(moved.hunks[0].id, ids[0], "unchanged hunk keeps its id across the rename");
    assert_ne!(moved.hunks[1].id, ids[1], "the edited hunk gets a new id");
}

fn decide(core: &mut Harness, wt: &Path, path: &str, decision: Option<Decision>) -> Vec<Event> {
    core.send(Command::DecideFile { worktree_path: path_str(wt), path: path.into(), decision })
}

fn commit(core: &mut Harness, wt: &Path, message: &str) -> Vec<Event> {
    core.send(Command::CommitAccepted { worktree_path: path_str(wt), message: message.into() })
}

fn refusal(events: &[Event]) -> String {
    match events {
        [Event::CommandRefused { reason, .. }] => reason.clone(),
        other => panic!("expected one refusal, got {other:#?}"),
    }
}

#[test]
fn deciding_a_file_is_reported_and_shown_on_the_next_read() {
    let (_repo, wt, mut core) = setup();
    std::fs::write(wt.join("lib.txt"), "changed\n").unwrap();

    let events = decide(&mut core, &wt, "lib.txt", Some(Decision::Accept));

    assert_eq!(
        events,
        vec![Event::FileDecided { worktree_path: path_str(&wt), path: "lib.txt".into(), decision: Some(Decision::Accept) }]
    );
    assert_eq!(file(&read(&mut core, &wt), "lib.txt").decision, Some(Decision::Accept));
    decide(&mut core, &wt, "lib.txt", None);
    assert_eq!(file(&read(&mut core, &wt), "lib.txt").decision, None);
}

#[test]
fn a_decision_goes_stale_when_the_file_changes_again() {
    let (_repo, wt, mut core) = setup();
    std::fs::write(wt.join("lib.txt"), "changed\n").unwrap();
    decide(&mut core, &wt, "lib.txt", Some(Decision::Accept));

    std::fs::write(wt.join("lib.txt"), "changed again\n").unwrap();

    let lib = file(&read(&mut core, &wt), "lib.txt").clone();
    assert_eq!((lib.decision, lib.stale), (Some(Decision::Accept), true));
    assert_eq!(
        refusal(&commit(&mut core, &wt, "msg")),
        "lib.txt changed since you decided; re-confirm it before committing"
    );
}

#[test]
fn deciding_an_unchanged_file_is_refused() {
    let (_repo, wt, mut core) = setup();
    assert_eq!(refusal(&decide(&mut core, &wt, "lib.txt", Some(Decision::Reject))), "lib.txt has no uncommitted changes");
}

#[test]
fn commit_takes_whole_accepted_files_and_leaves_the_rest_untouched() {
    let (_repo, wt, mut core) = setup();
    let lib = numbered(1..=40).replace("line 5\n", "line five\n").replace("line 30\n", "line thirty\n");
    std::fs::write(wt.join("lib.txt"), &lib).unwrap();
    std::fs::write(wt.join("new.txt"), "fresh\n").unwrap();
    std::fs::remove_file(wt.join("old.txt")).unwrap();
    std::fs::write(wt.join("README"), "undecided\n").unwrap();
    std::fs::write(wt.join("nope.txt"), "rejected\n").unwrap();
    for p in ["lib.txt", "new.txt", "old.txt"] {
        decide(&mut core, &wt, p, Some(Decision::Accept));
    }
    decide(&mut core, &wt, "nope.txt", Some(Decision::Reject));

    let events = commit(&mut core, &wt, "Add the thing\n\n- update lib.txt");

    let committed = events.iter().find_map(|e| match e {
        Event::Committed { branch, files, commit, .. } => Some((branch.clone(), files.clone(), commit.clone())),
        _ => None,
    });
    let (branch, mut files, sha) = committed.unwrap_or_else(|| panic!("no Committed in {events:#?}"));
    files.sort();
    assert_eq!(branch.as_deref(), Some("feat/x"));
    assert_eq!(files, vec!["lib.txt", "new.txt", "old.txt"]);
    assert_eq!(git_out(&wt, &["rev-parse", "--short", "HEAD"]).trim(), sha);
    // The commit holds exactly the accepted files, whole.
    let shown = git_out(&wt, &["show", "--name-status", "--format=%s", "HEAD"]);
    assert_eq!(
        shown.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
        vec!["Add the thing", "M\tlib.txt", "A\tnew.txt", "D\told.txt"]
    );
    assert_eq!(git_out(&wt, &["show", "HEAD:lib.txt"]), lib);
    // Nothing is left staged; the others are still uncommitted, as they were.
    assert_eq!(git_out(&wt, &["diff", "--cached", "--name-only"]), "");
    assert_eq!(std::fs::read_to_string(wt.join("README")).unwrap(), "undecided\n");
    assert_eq!(git_out(&wt, &["status", "--porcelain"]), " M README\n?? nope.txt\n");
    // Fresh changes after the commit are listed with no decisions.
    let after = events.iter().rev().find_map(|e| match e {
        Event::ChangesRead { files, .. } => Some(files.clone()),
        _ => None,
    });
    let after = after.expect("changes re-read after commit");
    assert_eq!(
        after.iter().map(|f| (f.path.as_str(), f.decision)).collect::<Vec<_>>(),
        vec![("README", None), ("nope.txt", None)]
    );
    assert!(events.iter().any(|e| matches!(e, Event::WorktreeFactsChanged { facts, .. } if facts.ahead == 1)));
}

#[test]
fn decisions_on_a_committed_file_are_gone_afterwards() {
    let (_repo, wt, mut core) = setup();
    std::fs::write(wt.join("lib.txt"), "one\n").unwrap();
    decide(&mut core, &wt, "lib.txt", Some(Decision::Accept));
    commit(&mut core, &wt, "first");

    std::fs::write(wt.join("lib.txt"), "two\n").unwrap();

    assert_eq!(file(&read(&mut core, &wt), "lib.txt").decision, None);
    assert_eq!(refusal(&commit(&mut core, &wt, "second")), "Accept at least one file to commit");
}

#[test]
fn a_renamed_file_commits_both_its_paths() {
    let (_repo, wt, mut core) = setup();
    std::fs::remove_file(wt.join("lib.txt")).unwrap();
    std::fs::write(wt.join("core.txt"), numbered(1..=40)).unwrap();
    decide(&mut core, &wt, "core.txt", Some(Decision::Accept));

    commit(&mut core, &wt, "Rename");

    assert_eq!(git_out(&wt, &["status", "--porcelain"]), "");
    let shown = git_out(&wt, &["show", "-M", "--name-status", "--format=", "HEAD"]);
    assert!(shown.trim_start().starts_with("R100\tlib.txt\tcore.txt"), "{shown}");
}

#[test]
fn commit_is_refused_without_a_message_or_an_accepted_file() {
    let (_repo, wt, mut core) = setup();
    std::fs::write(wt.join("lib.txt"), "x\n").unwrap();
    assert_eq!(refusal(&commit(&mut core, &wt, "msg")), "Accept at least one file to commit");
    decide(&mut core, &wt, "lib.txt", Some(Decision::Accept));
    assert_eq!(refusal(&commit(&mut core, &wt, "  \n ")), "Write a commit message first");
    assert_eq!(git_out(&wt, &["rev-list", "--count", "HEAD"]).trim(), "2");
}

#[test]
fn commit_is_refused_when_an_accepted_file_has_unreviewed_staged_changes() {
    let (_repo, wt, mut core) = setup();
    std::fs::write(wt.join("lib.txt"), "staged by someone\n").unwrap();
    git(&wt, &["add", "lib.txt"]);
    std::fs::write(wt.join("lib.txt"), "then edited\n").unwrap();
    decide(&mut core, &wt, "lib.txt", Some(Decision::Accept));

    let reason = refusal(&commit(&mut core, &wt, "msg"));

    assert_eq!(reason, "lib.txt has staged changes that were not reviewed; unstage them first");
    assert_eq!(git_out(&wt, &["rev-list", "--count", "HEAD"]).trim(), "2");
}

#[test]
fn a_conflicted_file_is_listed_as_conflicted_and_blocks_commit() {
    let (repo, wt, mut core) = setup();
    std::fs::write(wt.join("README"), "ours\n").unwrap();
    git(&wt, &["commit", "-qam", "ours"]);
    std::fs::write(repo.path.join("README"), "theirs\n").unwrap();
    git(&repo.path, &["commit", "-qam", "theirs"]);
    let merge = Process::new("git").args(["merge", "-q", "main"]).current_dir(&wt).output().unwrap();
    assert!(!merge.status.success(), "the merge conflicts");
    std::fs::write(wt.join("lib.txt"), "x\n").unwrap();

    let files = read(&mut core, &wt);

    assert_eq!(file(&files, "README").status, FileStatus::Conflicted);
    assert_eq!(
        refusal(&decide(&mut core, &wt, "README", Some(Decision::Accept))),
        "README has a conflict; resolve it before deciding"
    );
    decide(&mut core, &wt, "lib.txt", Some(Decision::Accept));
    assert_eq!(refusal(&commit(&mut core, &wt, "msg")), "Resolve the conflict in README before committing");
}

#[test]
fn a_file_that_conflicts_with_base_cannot_be_decided_and_blocks_commit() {
    let (repo, wt, mut core) = setup();
    // Base moves on and changes README; the worktree changes it too (uncommitted).
    std::fs::write(repo.path.join("README"), "theirs\n").unwrap();
    git(&repo.path, &["commit", "-qam", "theirs"]);
    std::fs::write(wt.join("README"), "ours\n").unwrap();
    git(&wt, &["commit", "-qam", "ours"]);
    std::fs::write(wt.join("README"), "ours, edited\n").unwrap();
    std::fs::write(wt.join("lib.txt"), "x\n").unwrap();

    assert_eq!(
        refusal(&decide(&mut core, &wt, "README", Some(Decision::Accept))),
        "README conflicts with main; rebase before deciding"
    );
    decide(&mut core, &wt, "lib.txt", Some(Decision::Accept));
    assert_eq!(refusal(&commit(&mut core, &wt, "msg")), "Resolve the conflict in README before committing");
    assert_eq!(git_out(&wt, &["rev-list", "--count", "HEAD"]).trim(), "3");
}

#[test]
fn review_commands_on_an_unknown_worktree_are_refused() {
    let mut core = Harness::launch();
    let events = core.send(Command::ReadChanges { worktree_path: "/nowhere".into() });
    assert_eq!(refusal(&events), "/nowhere is not a worktree of an added repository");
}
