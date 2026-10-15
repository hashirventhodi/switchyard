//! When a Worktree's watcher cannot run, the core says so and re-derives the Worktree's facts
//! on a slow cadence until a watcher works again (ADR-0008). The watcher is fake here:
//! `watcher_failed` makes it fail, `fallback_tick` is one beat of the slow cadence and
//! `watcher_recovered` lets it work again.
mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, Harness, TempRepo};
use switchyard_schema::{Command, Event};

/// A repository with one linked worktree on `feat/login`, added and attached.
fn attached() -> (TempRepo, PathBuf, Harness) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/login");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    core.send(Command::Attach);
    (repo, worktree, core)
}

fn path_of(worktree: &Path) -> String {
    worktree.to_string_lossy().into_owned()
}

#[test]
fn a_watcher_that_fails_marks_the_worktree_unwatched_with_the_reason() {
    let (_repo, worktree, mut core) = attached();

    let events = core.watcher_failed(&worktree, "inotify watch limit reached");

    assert_eq!(
        events,
        vec![Event::WorktreeUnwatched { worktree_path: path_of(&worktree), reason: "inotify watch limit reached".into() }]
    );
}

/// The dirty count pushed for `worktree` in `events` (exactly one facts event).
fn dirty_of(events: &[Event], worktree: &Path) -> u32 {
    let found: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorktreeFactsChanged { worktree_path, facts } if Path::new(worktree_path) == worktree => Some(facts.dirty),
            _ => None,
        })
        .collect();
    assert_eq!(found.len(), 1, "expected one facts event for {worktree:?} in {events:#?}");
    found[0]
}

#[test]
fn an_unwatched_worktree_still_gets_its_facts_on_the_slow_cadence() {
    let (_repo, worktree, mut core) = attached();
    core.watcher_failed(&worktree, "inotify watch limit reached");

    std::fs::write(worktree.join("README"), "edited while nothing watches\n").unwrap();
    let events = core.fallback_tick();

    assert_eq!(dirty_of(&events, &worktree), 1);
    assert!(!events.iter().any(|e| matches!(e, Event::WorktreeWatched { .. })), "{events:#?}");
}

#[test]
fn a_watched_worktree_is_left_alone_by_the_slow_cadence() {
    let (_repo, _worktree, mut core) = attached();

    assert_eq!(core.fallback_tick(), vec![]);
}

#[test]
fn a_recovered_watcher_clears_the_notice_and_ends_the_slow_cadence() {
    let (_repo, worktree, mut core) = attached();
    core.watcher_failed(&worktree, "inotify watch limit reached");

    std::fs::write(worktree.join("README"), "edited while nothing watches\n").unwrap();
    let events = core.watcher_recovered(&worktree);

    assert!(events.contains(&Event::WorktreeWatched { worktree_path: path_of(&worktree) }), "{events:#?}");
    assert_eq!(dirty_of(&events, &worktree), 1, "what changed while unwatched is caught up");
    assert_eq!(core.fallback_tick(), vec![]);
    let live = core.disk_changed(&worktree, &[&worktree.join("README")]);
    assert_eq!(dirty_of(&live, &worktree), 1, "live updates are back");
}

#[test]
fn a_watcher_that_cannot_start_leaves_the_worktree_unwatched() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/login");
    let mut core = Harness::launch();
    core.watcher_failed(&worktree, "network filesystems cannot be watched");
    core.send(Command::Attach);

    let events = core.add_repository(&repo.path);

    let unwatched =
        Event::WorktreeUnwatched { worktree_path: path_of(&worktree), reason: "network filesystems cannot be watched".into() };
    assert_eq!(events.iter().filter(|e| **e == unwatched).count(), 1, "{events:#?}");
    assert!(!events.iter().any(|e| matches!(e, Event::WorktreeUnwatched { worktree_path, .. } if Path::new(worktree_path) == repo.path)));
    assert_eq!(dirty_of(&core.fallback_tick(), &worktree), 0);
}

#[test]
fn attach_replays_the_worktrees_still_unwatched() {
    let (repo, worktree, mut core) = attached();
    core.watcher_failed(&worktree, "inotify watch limit reached");

    let events = core.send(Command::Attach);

    let unwatched = Event::WorktreeUnwatched { worktree_path: path_of(&worktree), reason: "inotify watch limit reached".into() };
    assert_eq!(events.iter().filter(|e| **e == unwatched).count(), 1, "{events:#?}");
    assert!(!events.iter().any(|e| matches!(e, Event::WorktreeUnwatched { worktree_path, .. } if Path::new(worktree_path) == repo.path)));
}

#[test]
fn a_removed_worktree_is_no_longer_re_derived() {
    let (_repo, worktree, mut core) = attached();
    core.watcher_failed(&worktree, "inotify watch limit reached");

    core.send(Command::RemoveWorktree { worktree_path: path_of(&worktree) });

    assert_eq!(core.fallback_tick(), vec![]);
}
