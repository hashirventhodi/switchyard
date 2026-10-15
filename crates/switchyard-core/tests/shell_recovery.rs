//! Shell PTYs are recorded by pid and start time like agents: after a crash, a shell still
//! running as the same process is killed on relaunch, never adopted. The shells are real
//! (`/bin/sh` in a PTY); the process table that judges them after the relaunch is the fake.

mod harness;

use std::path::{Path, PathBuf};

use harness::{Harness, TempRepo};
use switchyard_schema::{Command, Event};

fn launch() -> (TempRepo, PathBuf, Harness) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/x");
    let mut core = Harness::launch();
    core.use_shell("/bin/sh");
    core.add_repository(&repo.path);
    (repo, worktree, core)
}

/// Open a shell on `worktree`; returns its id and its pid (as the shell reports `$$`).
fn open(core: &mut Harness, worktree: &Path) -> (String, u32) {
    core.dispatch(Command::OpenShell { worktree_path: worktree.to_string_lossy().into_owned(), cols: 80, rows: 24 });
    let events = core.wait_for(|seen| seen.iter().any(|e| matches!(e, Event::ShellOpened { .. })));
    let id = events
        .iter()
        .find_map(|e| match e {
            Event::ShellOpened { shell } => Some(shell.id.clone()),
            _ => None,
        })
        .unwrap();
    core.dispatch(Command::WriteShell { shell_id: id.clone(), data: "echo \"pid=$((0+$$))=\"\n".into() });
    let output = |seen: &[Event]| -> String {
        seen.iter()
            .filter_map(|e| match e {
                Event::ShellOutput { data, .. } => Some(data.as_str()),
                _ => None,
            })
            .collect()
    };
    let seen = core.wait_for(|seen| printed_pid(&output(seen)).is_some());
    let pid = printed_pid(&output(&seen)).unwrap();
    (id, pid)
}

/// The number in the first `pid=<digits>=` (the echoed command line has no digits there).
fn printed_pid(text: &str) -> Option<u32> {
    text.split("pid=").skip(1).find_map(|part| part.split('=').next()?.parse().ok())
}

#[test]
fn a_shell_left_running_by_a_crash_is_killed_on_relaunch() {
    let (_repo, worktree, mut core) = launch();
    let (_, pid) = open(&mut core, &worktree);

    let core = core.relaunch();

    assert_eq!(core.processes.killed(), vec![pid]);
}

#[test]
fn a_reused_shell_pid_is_left_alone() {
    let (_repo, worktree, mut core) = launch();
    let (_, pid) = open(&mut core, &worktree);
    core.processes.reuse_pid(pid);

    let core = core.relaunch();

    assert!(core.processes.killed().is_empty());
}

#[test]
fn a_closed_shell_is_forgotten() {
    let (_repo, worktree, mut core) = launch();
    let (id, _) = open(&mut core, &worktree);
    core.dispatch(Command::CloseShell { shell_id: id });
    core.wait_for(|seen| seen.iter().any(|e| matches!(e, Event::ShellClosed { .. })));

    let core = core.relaunch();

    assert!(core.processes.killed().is_empty());
}
