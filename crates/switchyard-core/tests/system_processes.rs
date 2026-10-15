//! The real process table (`ps` / `kill`) that crash recovery uses, against a real
//! `sleep` process: a running pid has a start time, and killing it ends it.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use switchyard_core::processes::{ProcessTable, SystemProcesses};

#[test]
fn a_running_process_has_a_stable_start_time_and_is_gone_once_killed() {
    let mut child = Command::new("sleep").arg("30").spawn().expect("sleep runs");
    let pid = child.id();
    let table = SystemProcesses;

    let started = table.start_time(pid).expect("a running process has a start time");
    assert_eq!(table.start_time(pid).as_deref(), Some(started.as_str()));

    table.kill(pid);
    let status = child.wait().expect("the child is reaped");

    assert!(!status.success());
    assert_eq!(table.start_time(pid), None);
}

#[test]
fn killing_a_process_group_leader_ends_what_it_started() {
    // Agents run in their own process group (the sidecar and the CLI it starts).
    let mut leader = Command::new("sh")
        .args(["-c", "sleep 30 & echo $!; wait"])
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("sh runs");
    let mut line = String::new();
    BufReader::new(leader.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let grandchild: u32 = line.trim().parse().expect("sh printed the sleep's pid");
    let table = SystemProcesses;
    assert!(table.start_time(grandchild).is_some());

    table.kill(leader.id());
    leader.wait().unwrap();

    let gone = (0..100).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        table.start_time(grandchild).is_none()
    });
    assert!(gone, "the leader's child was killed with it");
}
