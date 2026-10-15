//! Stale decisions through seam 1 (ADR-0008): a Decision is pinned to the content it was
//! made on. When the file or hunk changes after you decided, the Decision stays, flagged as
//! a Stale decision, and stops counting until re-confirmed. The agent's edits arrive as a
//! disk change (`disk_changed`, the fake watcher); the Changes view then re-reads.
mod harness;

use std::path::{Path, PathBuf};

use adapter_protocol::Event as AgentEvent;
use harness::{git, Harness, TempRepo};
use switchyard_schema::{ChangedFile, Command, Decision, Event, Hunk, StaleHunk};

/// A repository with a committer identity and a worktree on `feat/x` whose `lib.txt` has two
/// hunks (line 5 and line 30 changed) and whose `notes.txt` has one, added to a core.
fn setup() -> (TempRepo, PathBuf, Harness) {
    let repo = TempRepo::new("acme");
    git(&repo.path, &["config", "user.name", "Test"]);
    git(&repo.path, &["config", "user.email", "test@example.com"]);
    std::fs::write(repo.path.join("lib.txt"), numbered(1..=40)).unwrap();
    std::fs::write(repo.path.join("notes.txt"), "a\nb\n").unwrap();
    git(&repo.path, &["add", "."]);
    git(&repo.path, &["commit", "-q", "-m", "seed"]);
    let wt = repo.add_worktree("feat/x");
    std::fs::write(wt.join("lib.txt"), lib(&["line five", "line thirty"])).unwrap();
    std::fs::write(wt.join("notes.txt"), "a\nB\n").unwrap();
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    (repo, wt, core)
}

fn numbered(range: std::ops::RangeInclusive<u32>) -> String {
    range.map(|n| format!("line {n}\n")).collect()
}

/// `lib.txt` with line 5 and line 30 replaced by `with[0]` and `with[1]`.
fn lib(with: &[&str; 2]) -> String {
    numbered(1..=40).replace("line 5\n", &format!("{}\n", with[0])).replace("line 30\n", &format!("{}\n", with[1]))
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The agent (or anyone) writes `path` in the worktree; the watcher reports it.
fn edit(core: &mut Harness, wt: &Path, path: &str, content: &str) {
    std::fs::write(wt.join(path), content).unwrap();
    core.disk_changed(wt, &[&wt.join(path)]);
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

fn file(core: &mut Harness, wt: &Path, path: &str) -> ChangedFile {
    let files = read(core, wt);
    files.into_iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path} not in the changes"))
}

fn decide(core: &mut Harness, wt: &Path, path: &str, decision: Option<Decision>) -> Vec<Event> {
    core.send(Command::DecideFile { worktree_path: path_str(wt), path: path.into(), decision })
}

fn decide_hunk(core: &mut Harness, wt: &Path, path: &str, hunk: &Hunk, decision: Option<Decision>) -> Vec<Event> {
    core.send(Command::DecideHunk { worktree_path: path_str(wt), path: path.into(), hunk_id: hunk.id.clone(), decision })
}

fn commit(core: &mut Harness, wt: &Path) -> Vec<Event> {
    core.send(Command::CommitAccepted { worktree_path: path_str(wt), message: "msg".into() })
}

fn refusal(events: &[Event]) -> String {
    match events {
        [Event::CommandRefused { reason, .. }] => reason.clone(),
        other => panic!("expected one refusal, got {other:#?}"),
    }
}

#[test]
fn an_accept_on_a_file_edited_afterwards_is_kept_and_flagged_stale() {
    let (_repo, wt, mut core) = setup();
    decide(&mut core, &wt, "notes.txt", Some(Decision::Accept));
    assert!(!file(&mut core, &wt, "notes.txt").stale);

    edit(&mut core, &wt, "notes.txt", "a\nBee\n");

    let notes = file(&mut core, &wt, "notes.txt");
    assert_eq!((notes.decision, notes.stale), (Some(Decision::Accept), true));
}

#[test]
fn a_file_reverted_to_the_content_decided_on_is_not_stale_again() {
    let (_repo, wt, mut core) = setup();
    decide(&mut core, &wt, "notes.txt", Some(Decision::Accept));
    edit(&mut core, &wt, "notes.txt", "a\nBee\n");

    edit(&mut core, &wt, "notes.txt", "a\nB\n");

    let notes = file(&mut core, &wt, "notes.txt");
    assert_eq!((notes.decision, notes.stale), (Some(Decision::Accept), false));
}

#[test]
fn re_confirming_a_stale_decision_pins_it_to_the_current_content() {
    let (_repo, wt, mut core) = setup();
    decide(&mut core, &wt, "notes.txt", Some(Decision::Accept));
    edit(&mut core, &wt, "notes.txt", "a\nBee\n");

    decide(&mut core, &wt, "notes.txt", Some(Decision::Accept));

    let notes = file(&mut core, &wt, "notes.txt");
    assert_eq!((notes.decision, notes.stale), (Some(Decision::Accept), false));
    let events = commit(&mut core, &wt);
    assert!(events.iter().any(|e| matches!(e, Event::Committed { files, .. } if files == &vec!["notes.txt".to_string()])));
}

#[test]
fn commit_accepted_skips_a_file_whose_accept_is_stale_and_says_so() {
    let (_repo, wt, mut core) = setup();
    decide(&mut core, &wt, "notes.txt", Some(Decision::Accept));
    decide(&mut core, &wt, "lib.txt", Some(Decision::Accept));
    edit(&mut core, &wt, "notes.txt", "a\nBee\n");

    let events = commit(&mut core, &wt);

    let committed = events.iter().find_map(|e| match e {
        Event::Committed { files, skipped_stale, .. } => Some((files.clone(), skipped_stale.clone())),
        _ => None,
    });
    assert_eq!(committed, Some((vec!["lib.txt".to_string()], vec!["notes.txt".to_string()])), "{events:#?}");
    assert_eq!(std::fs::read_to_string(wt.join("notes.txt")).unwrap(), "a\nBee\n");
    let notes = file(&mut core, &wt, "notes.txt");
    assert_eq!((notes.decision, notes.stale), (None, false), "decisions are discarded on commit");
}

#[test]
fn commit_with_only_stale_accepts_is_refused_naming_them() {
    let (_repo, wt, mut core) = setup();
    decide(&mut core, &wt, "notes.txt", Some(Decision::Accept));
    edit(&mut core, &wt, "notes.txt", "a\nBee\n");

    assert_eq!(
        refusal(&commit(&mut core, &wt)),
        "notes.txt changed since you decided; re-confirm it before committing"
    );
}

#[test]
fn a_hunk_decision_stays_in_force_when_another_hunk_in_the_file_changes() {
    let (_repo, wt, mut core) = setup();
    let hunks = file(&mut core, &wt, "lib.txt").hunks;
    decide_hunk(&mut core, &wt, "lib.txt", &hunks[0], Some(Decision::Accept));

    edit(&mut core, &wt, "lib.txt", &lib(&["line five", "line 30 (kept)"]));

    let after = file(&mut core, &wt, "lib.txt");
    assert_eq!(after.hunks[0].id, hunks[0].id);
    assert_eq!(after.hunks[0].decision, Some(Decision::Accept));
    assert!(after.stale_hunks.is_empty(), "{:#?}", after.stale_hunks);
}

#[test]
fn a_rejected_hunk_whose_feedback_was_sent_reads_revised_once_the_agent_changes_it() {
    let (_repo, wt, mut core) = setup();
    harness::started_session(&core.start_session(&wt, "Tidy the library."));
    core.agent_says(0, AgentEvent::TurnComplete);
    let hunks = file(&mut core, &wt, "lib.txt").hunks;
    decide_hunk(&mut core, &wt, "lib.txt", &hunks[1], Some(Decision::Reject));
    core.send(Command::SendFeedback { worktree_path: path_str(&wt) });

    edit(&mut core, &wt, "lib.txt", &lib(&["line five", "line 30 (kept)"]));

    let after = file(&mut core, &wt, "lib.txt");
    assert_eq!(after.hunks[1].decision, None, "the revision is a new hunk, undecided");
    assert_eq!(
        after.stale_hunks,
        vec![StaleHunk { id: hunks[1].id.clone(), header: hunks[1].header.clone(), decision: Decision::Reject, comment: None, sent: true }]
    );
}

#[test]
fn a_stale_hunk_decision_survives_a_relaunch_and_is_discarded_on_commit() {
    let (_repo, wt, mut core) = setup();
    let hunks = file(&mut core, &wt, "lib.txt").hunks;
    decide_hunk(&mut core, &wt, "lib.txt", &hunks[1], Some(Decision::Reject));
    edit(&mut core, &wt, "lib.txt", &lib(&["line five", "line 30 (kept)"]));

    let mut core = core.relaunch();

    let stale = file(&mut core, &wt, "lib.txt").stale_hunks;
    assert_eq!(stale.iter().map(|h| (h.id.clone(), h.decision, h.sent)).collect::<Vec<_>>(), vec![(hunks[1].id.clone(), Decision::Reject, false)]);
    decide(&mut core, &wt, "notes.txt", Some(Decision::Accept));
    commit(&mut core, &wt);
    assert!(file(&mut core, &wt, "lib.txt").stale_hunks.is_empty());
}
