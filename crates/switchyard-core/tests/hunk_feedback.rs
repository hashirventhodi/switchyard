//! Hunk feedback through seam 1: Accept / Reject / Comment per hunk, rejected hunks
//! blocking their file's commit, and feedback sent to the agent as one message. Switchyard
//! never changes the tree. Real temporary git repositories throughout.
mod harness;

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use adapter_protocol::{Command as AgentCommand, Event as AgentEvent};
use harness::{git, Harness, TempRepo};
use switchyard_schema::{ChangedFile, Command, Decision, Entry, Event, Hunk, SessionStatus};

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
    std::fs::write(
        wt.join("lib.txt"),
        numbered(1..=40).replace("line 5\n", "line five\n").replace("line 30\n", "line thirty\n"),
    )
    .unwrap();
    std::fs::write(wt.join("notes.txt"), "a\nB\n").unwrap();
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
    core.send(Command::ReadChanges { worktree_path: path_str(wt) })
        .into_iter()
        .find_map(|e| match e {
            Event::ChangesRead { files, .. } => Some(files),
            _ => None,
        })
        .expect("ChangesRead")
}

fn file(files: &[ChangedFile], path: &str) -> ChangedFile {
    files.iter().find(|f| f.path == path).cloned().unwrap_or_else(|| panic!("{path} not in {files:#?}"))
}

fn hunks(core: &mut Harness, wt: &Path, path: &str) -> Vec<Hunk> {
    file(&read(core, wt), path).hunks
}

fn decide_hunk(core: &mut Harness, wt: &Path, path: &str, hunk: &Hunk, decision: Option<Decision>) -> Vec<Event> {
    core.send(Command::DecideHunk { worktree_path: path_str(wt), path: path.into(), hunk_id: hunk.id.clone(), decision })
}

fn comment(core: &mut Harness, wt: &Path, path: &str, hunk: &Hunk, text: Option<&str>) -> Vec<Event> {
    core.send(Command::CommentHunk {
        worktree_path: path_str(wt),
        path: path.into(),
        hunk_id: hunk.id.clone(),
        comment: text.map(Into::into),
    })
}

fn decide_file(core: &mut Harness, wt: &Path, path: &str, decision: Option<Decision>) -> Vec<Event> {
    core.send(Command::DecideFile { worktree_path: path_str(wt), path: path.into(), decision })
}

fn commit(core: &mut Harness, wt: &Path) -> Vec<Event> {
    core.send(Command::CommitAccepted { worktree_path: path_str(wt), message: "msg".into() })
}

fn send_feedback(core: &mut Harness, wt: &Path) -> Vec<Event> {
    core.send(Command::SendFeedback { worktree_path: path_str(wt) })
}

fn refusal(events: &[Event]) -> String {
    match events {
        [Event::CommandRefused { reason, .. }] => reason.clone(),
        other => panic!("expected one refusal, got {other:#?}"),
    }
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Process::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn committed_files(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .find_map(|e| match e {
            Event::Committed { files, .. } => Some(files.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no Committed in {events:#?}"))
}

#[test]
fn a_rejected_hunk_blocks_commit_for_its_file_with_a_reason() {
    let (_repo, wt, mut core) = setup();
    let lib = hunks(&mut core, &wt, "lib.txt");
    let head = git_out(&wt, &["rev-parse", "HEAD"]);

    let events = decide_hunk(&mut core, &wt, "lib.txt", &lib[0], Some(Decision::Reject));

    assert_eq!(
        events,
        vec![Event::HunkReviewed {
            worktree_path: path_str(&wt),
            path: "lib.txt".into(),
            hunk_id: lib[0].id.clone(),
            decision: Some(Decision::Reject),
            comment: None,
            sent: false,
        }]
    );
    assert_eq!(hunks(&mut core, &wt, "lib.txt")[0].decision, Some(Decision::Reject));
    assert_eq!(
        refusal(&commit(&mut core, &wt)),
        "lib.txt has a rejected hunk: send feedback to the agent or accept the whole file"
    );
    assert_eq!(git_out(&wt, &["rev-parse", "HEAD"]), head);
}

#[test]
fn commit_leaves_out_a_file_with_a_rejected_hunk_and_takes_a_file_whose_hunks_are_all_accepted() {
    let (_repo, wt, mut core) = setup();
    let lib = hunks(&mut core, &wt, "lib.txt");
    let notes = hunks(&mut core, &wt, "notes.txt");
    decide_hunk(&mut core, &wt, "lib.txt", &lib[0], Some(Decision::Accept));
    decide_hunk(&mut core, &wt, "lib.txt", &lib[1], Some(Decision::Reject));
    decide_hunk(&mut core, &wt, "notes.txt", &notes[0], Some(Decision::Accept));

    let events = commit(&mut core, &wt);

    assert_eq!(committed_files(&events), vec!["notes.txt"]);
    assert_eq!(git_out(&wt, &["status", "--porcelain"]), " M lib.txt\n");
}

#[test]
fn rejecting_a_hunk_takes_back_an_accepted_file_and_accepting_the_file_clears_the_rejection() {
    let (_repo, wt, mut core) = setup();
    let lib = hunks(&mut core, &wt, "lib.txt");
    decide_file(&mut core, &wt, "lib.txt", Some(Decision::Accept));

    let events = decide_hunk(&mut core, &wt, "lib.txt", &lib[1], Some(Decision::Reject));

    assert_eq!(
        events[0],
        Event::FileDecided { worktree_path: path_str(&wt), path: "lib.txt".into(), decision: None }
    );
    assert_eq!(file(&read(&mut core, &wt), "lib.txt").decision, None);

    let events = decide_file(&mut core, &wt, "lib.txt", Some(Decision::Accept));

    assert!(events.contains(&Event::HunkReviewed {
        worktree_path: path_str(&wt),
        path: "lib.txt".into(),
        hunk_id: lib[1].id.clone(),
        decision: None,
        comment: None,
        sent: false,
    }));
    assert!(hunks(&mut core, &wt, "lib.txt").iter().all(|h| h.decision.is_none()));
    assert_eq!(committed_files(&commit(&mut core, &wt)), vec!["lib.txt"]);
}

/// Start a session on the worktree and let its first turn finish (it waits for the user).
fn waiting_session(core: &mut Harness, wt: &Path) -> String {
    let id = harness::started_session(&core.start_session(wt, "Tidy the library."));
    core.agent_says(0, AgentEvent::TurnComplete);
    id
}

fn sent_texts(core: &Harness, from: usize) -> Vec<String> {
    core.agents.commands(0)[from..]
        .iter()
        .map(|c| match c {
            AgentCommand::SendText { text } => text.clone(),
            other => panic!("unexpected agent command {other:?}"),
        })
        .collect()
}

#[test]
fn feedback_goes_to_the_agent_as_one_message_naming_each_rejected_or_commented_hunk_once() {
    let (_repo, wt, mut core) = setup();
    let id = waiting_session(&mut core, &wt);
    let lib = hunks(&mut core, &wt, "lib.txt");
    let notes = hunks(&mut core, &wt, "notes.txt");
    decide_hunk(&mut core, &wt, "lib.txt", &lib[1], Some(Decision::Reject));
    comment(&mut core, &wt, "lib.txt", &lib[1], Some("Keep the number; words are harder to grep."));
    comment(&mut core, &wt, "notes.txt", &notes[0], Some("Why uppercase?"));
    decide_hunk(&mut core, &wt, "lib.txt", &lib[0], Some(Decision::Accept));
    let snapshot = harness::worktree_snapshot(&wt);
    let index = git_out(&wt, &["ls-files", "-s"]);
    let before = core.agents.commands(0).len();

    let events = send_feedback(&mut core, &wt);

    let texts = sent_texts(&core, before);
    let [text] = texts.as_slice() else { panic!("expected one message, got {texts:#?}") };
    assert_eq!(text.matches("## lib.txt").count(), 1, "{text}");
    assert_eq!(text.matches("## notes.txt").count(), 1, "{text}");
    assert_eq!(text.matches(&lib[1].header).count(), 1, "{text}");
    assert!(text.contains(&format!("Rejected hunk `{}`", lib[1].header)), "{text}");
    assert!(!text.contains(&lib[0].header), "an accepted hunk is not feedback: {text}");
    assert_eq!(text.matches("Keep the number; words are harder to grep.").count(), 1, "{text}");
    assert!(text.contains("-line 30\n+line thirty"), "{text}");
    assert!(text.contains(&format!("Comment on hunk `{}`", notes[0].header)), "{text}");
    assert!(text.contains("Why uppercase?"), "{text}");
    assert!(events.contains(&Event::SessionStatusChanged { session_id: id.clone(), status: SessionStatus::Running }));
    assert!(events.iter().any(|e| matches!(e, Event::TranscriptEntry { entry: Entry::User { text: t }, .. } if t == text)));
    assert_eq!(events.last(), Some(&Event::FeedbackSent { worktree_path: path_str(&wt), session_id: id, hunks: 2 }));
    let marked: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            Event::HunkReviewed { hunk_id, sent: true, .. } => Some(hunk_id),
            _ => None,
        })
        .collect();
    assert_eq!(marked, vec![&lib[1].id, &notes[0].id]);
    // Switchyard changed nothing in the tree or the index.
    assert_eq!(harness::worktree_snapshot(&wt), snapshot);
    assert_eq!(git_out(&wt, &["ls-files", "-s"]), index);
    // The pending set is empty now, and the rejection still blocks its file.
    core.agent_says(0, AgentEvent::TurnComplete);
    assert_eq!(refusal(&send_feedback(&mut core, &wt)), "No hunk feedback to send: reject or comment on a hunk first");
    let after = hunks(&mut core, &wt, "lib.txt");
    assert_eq!((after[1].decision, after[1].sent), (Some(Decision::Reject), true));
    assert_eq!(
        refusal(&commit(&mut core, &wt)),
        "lib.txt has a rejected hunk: send feedback to the agent or accept the whole file"
    );
}

#[test]
fn a_new_comment_on_a_sent_hunk_is_pending_again() {
    let (_repo, wt, mut core) = setup();
    waiting_session(&mut core, &wt);
    let lib = hunks(&mut core, &wt, "lib.txt");
    decide_hunk(&mut core, &wt, "lib.txt", &lib[1], Some(Decision::Reject));
    send_feedback(&mut core, &wt);
    core.agent_says(0, AgentEvent::TurnComplete);
    comment(&mut core, &wt, "lib.txt", &lib[1], Some("Still wrong."));
    let before = core.agents.commands(0).len();

    let events = send_feedback(&mut core, &wt);

    assert!(matches!(events.last(), Some(Event::FeedbackSent { hunks: 1, .. })), "{events:#?}");
    let texts = sent_texts(&core, before);
    assert!(texts[0].contains("Still wrong.") && texts[0].contains("Rejected hunk"), "{texts:#?}");
}

#[test]
fn when_the_agent_revises_a_rejected_hunk_the_rejection_lapses() {
    let (_repo, wt, mut core) = setup();
    let lib = hunks(&mut core, &wt, "lib.txt");
    decide_hunk(&mut core, &wt, "lib.txt", &lib[0], Some(Decision::Accept));
    decide_hunk(&mut core, &wt, "lib.txt", &lib[1], Some(Decision::Reject));

    // The agent rewrites the rejected hunk; the accepted one is untouched.
    std::fs::write(
        wt.join("lib.txt"),
        numbered(1..=40).replace("line 5\n", "line five\n").replace("line 30\n", "line 30 (kept)\n"),
    )
    .unwrap();

    let after = hunks(&mut core, &wt, "lib.txt");
    assert_eq!(after[0].id, lib[0].id);
    assert_eq!(after[0].decision, Some(Decision::Accept));
    assert_ne!(after[1].id, lib[1].id);
    assert_eq!(after[1].decision, None);
    assert_eq!(refusal(&commit(&mut core, &wt)), "Accept at least one file to commit");
}

#[test]
fn sending_feedback_is_refused_without_a_session_ready_for_it_and_nothing_is_marked_sent() {
    let (_repo, wt, mut core) = setup();
    let lib = hunks(&mut core, &wt, "lib.txt");
    decide_hunk(&mut core, &wt, "lib.txt", &lib[1], Some(Decision::Reject));

    assert_eq!(refusal(&send_feedback(&mut core, &wt)), "No session on this worktree to send feedback to");

    core.start_session(&wt, "Tidy the library.");
    assert_eq!(
        refusal(&send_feedback(&mut core, &wt)),
        "The agent is still working; wait for it or stop it first"
    );
    assert!(!hunks(&mut core, &wt, "lib.txt")[1].sent);
    assert_eq!(core.agents.commands(0).len(), 1, "only the first prompt was sent");
}

#[test]
fn a_comment_can_be_cleared_and_unknown_hunks_are_refused() {
    let (_repo, wt, mut core) = setup();
    let lib = hunks(&mut core, &wt, "lib.txt");
    comment(&mut core, &wt, "lib.txt", &lib[0], Some("hm"));

    let events = comment(&mut core, &wt, "lib.txt", &lib[0], Some("   "));

    assert!(matches!(&events[..], [Event::HunkReviewed { comment: None, decision: None, .. }]), "{events:#?}");
    assert_eq!(hunks(&mut core, &wt, "lib.txt")[0].comment, None);
    let gone = Hunk { id: "0000".into(), ..lib[0].clone() };
    assert_eq!(
        refusal(&decide_hunk(&mut core, &wt, "lib.txt", &gone, Some(Decision::Reject))),
        "That hunk is no longer in lib.txt; the agent may have changed it"
    );
}

/// Switchyard never reverts or partially stages: no git invocation in the core names a
/// verb that rewrites the working tree from the index or a patch.
#[test]
fn no_core_code_path_calls_checkout_or_apply_on_the_working_tree() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut dirs = vec![src];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            // The dangerous-command classifier names these verbs to recognise them in the
            // agent's commands; it never runs git.
            if path.ends_with("policy/dangerous.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for verb in ["\"checkout\"", "\"apply\"", "\"restore\"", "\"stash\"", "\"--hard\"", "\"clean\""] {
                if text.contains(verb) {
                    offenders.push(format!("{} uses {verb}", path.display()));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}

#[test]
fn hunk_reviews_survive_a_relaunch() {
    let (_repo, wt, mut core) = setup();
    let lib = hunks(&mut core, &wt, "lib.txt");
    decide_hunk(&mut core, &wt, "lib.txt", &lib[0], Some(Decision::Reject));
    comment(&mut core, &wt, "lib.txt", &lib[0], Some("use a constant"));
    decide_hunk(&mut core, &wt, "lib.txt", &lib[1], Some(Decision::Accept));
    let notes = hunks(&mut core, &wt, "notes.txt");
    comment(&mut core, &wt, "notes.txt", &notes[0], Some("why uppercase?"));
    comment(&mut core, &wt, "notes.txt", &notes[0], None);

    let mut core = core.relaunch();

    let lib = hunks(&mut core, &wt, "lib.txt");
    assert_eq!(lib[0].decision, Some(Decision::Reject));
    assert_eq!(lib[0].comment.as_deref(), Some("use a constant"));
    assert!(!lib[0].sent);
    assert_eq!(lib[1].decision, Some(Decision::Accept));
    assert_eq!(lib[1].comment, None);
    let notes = hunks(&mut core, &wt, "notes.txt");
    assert_eq!((notes[0].decision, notes[0].comment.clone()), (None, None));
}
