//! Shell terminals in the dock: real PTYs running in a worktree, driven through seam 1.
//! The shell is `/bin/sh` with no rc files, so output is the same on every machine.

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

fn open(core: &mut Harness, worktree: &Path) -> String {
    core.dispatch(Command::OpenShell { worktree_path: worktree.to_string_lossy().into_owned(), cols: 80, rows: 24 });
    let events = core.wait_for(|seen| seen.iter().any(|e| matches!(e, Event::ShellOpened { .. })));
    events
        .iter()
        .find_map(|e| match e {
            Event::ShellOpened { shell } => {
                assert_eq!(shell.worktree_path, worktree.to_string_lossy());
                Some(shell.id.clone())
            }
            _ => None,
        })
        .unwrap()
}

fn write(core: &mut Harness, shell: &str, data: &str) {
    core.dispatch(Command::WriteShell { shell_id: shell.into(), data: data.into() });
}

fn output_of(shell: &str, seen: &[Event]) -> String {
    seen.iter()
        .filter_map(|e| match e {
            Event::ShellOutput { shell_id, data } if shell_id == shell => Some(data.as_str()),
            _ => None,
        })
        .collect()
}

/// Collect a shell's output until it contains `needle`; returns everything read.
fn output_until(core: &mut Harness, shell: &str, needle: &str) -> String {
    let events = core.wait_for(|seen| output_of(shell, seen).contains(needle));
    output_of(shell, &events)
}

/// The shell's size as `stty` reports it, asked for with a numbered marker so the answer is
/// not confused with an earlier one. Returns once the next prompt is up: a resize that lands while
/// sh is still finishing a command can be lost to the shell itself (seen with macOS sh 3.2).
fn stty_size(core: &mut Harness, shell: &str, n: u32) -> String {
    // The typed line echoes back as `$((1000+n))`; only the answer contains `size100n:`.
    write(core, shell, &format!("echo \"size$((1000+{n})):$(stty size):\"\n"));
    let marker = format!("size{}:", 1000 + n);
    let answer = |text: &str| text.split_once(&marker).and_then(|(_, rest)| rest.split_once(':')).map(|(a, _)| a.to_string());
    let prompt_after = |text: &str| text.split_once(&marker).is_some_and(|(_, rest)| rest.contains("$ "));
    let events = core.wait_for(|seen| prompt_after(&output_of(shell, seen)));
    answer(&output_of(shell, &events)).unwrap()
}

#[test]
fn a_shell_runs_interactive_commands_in_its_worktree() {
    let (_repo, worktree, mut core) = launch();
    let shell = open(&mut core, &worktree);

    write(&mut core, &shell, "echo \"in:$(pwd -P)\"\n");

    output_until(&mut core, &shell, &format!("in:{}", worktree.canonicalize().unwrap().display()));
}

#[test]
fn a_shell_takes_the_size_of_its_terminal_and_follows_resizes() {
    let (_repo, worktree, mut core) = launch();
    let shell = open(&mut core, &worktree);
    assert_eq!(stty_size(&mut core, &shell, 1), "24 80");

    core.dispatch(Command::ResizeShell { shell_id: shell.clone(), cols: 132, rows: 40 });

    assert_eq!(stty_size(&mut core, &shell, 2), "40 132");
}

fn closed(events: &[Event], shell: &str) -> Option<Option<u32>> {
    events.iter().find_map(|e| match e {
        Event::ShellClosed { shell_id, exit_code } if shell_id == shell => Some(*exit_code),
        _ => None,
    })
}

#[test]
fn a_shell_that_exits_closes_with_its_exit_code() {
    let (_repo, worktree, mut core) = launch();
    let shell = open(&mut core, &worktree);

    write(&mut core, &shell, "exit 3\n");

    let events = core.wait_for(|seen| closed(seen, &shell).is_some());
    assert_eq!(closed(&events, &shell), Some(Some(3)));
}

#[test]
fn closing_a_shell_kills_its_shell() {
    let (_repo, worktree, mut core) = launch();
    let shell = open(&mut core, &worktree);
    write(&mut core, &shell, "echo \"pid:$$:\"\n");
    let out = output_until(&mut core, &shell, ":\r\n");
    let pid = out.split("pid:").last().unwrap().split(':').next().unwrap().to_string();

    let events = core.send(Command::CloseShell { shell_id: shell.clone() });

    assert_eq!(closed(&events, &shell), Some(None));
    assert!(gone(&pid), "shell {pid} still running");
    // Typing into a closed shell is refused.
    let events = core.send(Command::WriteShell { shell_id: shell.clone(), data: "ls\n".into() });
    assert!(matches!(&events[..], [Event::CommandRefused { .. }]), "{events:#?}");
}

/// The process is gone (polls briefly: a hung-up shell takes a moment to exit).
fn gone(pid: &str) -> bool {
    (0..500).any(|_| {
        let alive = std::process::Command::new("kill").args(["-0", pid]).status().unwrap().success();
        if alive {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        !alive
    })
}

#[test]
fn closing_a_worktree_closes_its_shells_and_no_others() {
    let (repo, worktree, mut core) = launch();
    let other = repo.add_worktree("feat/y");
    let a = open(&mut core, &worktree);
    let b = open(&mut core, &worktree);
    let c = open(&mut core, &other);

    let events = core.send(Command::CloseWorktreeShells { worktree_path: worktree.to_string_lossy().into_owned() });

    assert!(closed(&events, &a).is_some() && closed(&events, &b).is_some(), "{events:#?}");
    assert_eq!(closed(&events, &c), None);
    write(&mut core, &c, "echo still-$((1+1))\n");
    output_until(&mut core, &c, "still-2");
}

#[test]
fn a_shell_opens_only_on_a_known_worktree() {
    let (_repo, _worktree, mut core) = launch();
    let elsewhere = tempfile::TempDir::new().unwrap();

    let events = core.send(Command::OpenShell {
        worktree_path: elsewhere.path().to_string_lossy().into_owned(),
        cols: 80,
        rows: 24,
    });

    assert!(
        matches!(&events[..], [Event::CommandRefused { command, reason }] if command == "openShell" && reason.contains("not a worktree")),
        "{events:#?}"
    );
}

#[test]
fn attach_replays_open_shells_with_their_recent_output() {
    let (_repo, worktree, mut core) = launch();
    let shell = open(&mut core, &worktree);
    write(&mut core, &shell, "echo hello-$((40+2))\n");
    output_until(&mut core, &shell, "hello-42");

    let events = core.send(Command::Attach);

    let opened = events.iter().position(|e| matches!(e, Event::ShellOpened { shell: s } if s.id == shell));
    let output = events
        .iter()
        .position(|e| matches!(e, Event::ShellOutput { shell_id, data } if *shell_id == shell && data.contains("hello-42")));
    assert!(opened.is_some() && output > opened, "{events:#?}");
}
