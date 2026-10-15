//! The real process table (`ps` / `kill`) that crash recovery uses, against a real
//! `sleep` process: a running pid has a start time, and killing it ends it.

use std::process::Command;

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
