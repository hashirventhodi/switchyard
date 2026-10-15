//! The machine's process table, as far as crash recovery needs it. Every process the core
//! starts is recorded by pid and start time; on relaunch a recorded process still running
//! with the same start time is an orphan and is killed, never adopted. A pid whose start
//! time differs belongs to some other program now and is left alone.

use std::process::Command;

pub trait ProcessTable: Send + Sync {
    /// When the process `pid` started, in whatever form the platform reports it (compared
    /// for equality only); `None` when no such process runs.
    fn start_time(&self, pid: u32) -> Option<String>;
    /// Kill the process `pid` outright.
    fn kill(&self, pid: u32);
}

/// The real process table, read with `ps` and signalled with `kill` (macOS and Linux).
pub struct SystemProcesses;

impl ProcessTable for SystemProcesses {
    fn start_time(&self, pid: u32) -> Option<String> {
        let out = Command::new("ps").args(["-o", "lstart=", "-p", &pid.to_string()]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !text.is_empty()).then_some(text)
    }

    fn kill(&self, pid: u32) {
        let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).output();
    }
}

/// A process the core started: enough to recognise it after a crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recorded {
    pub pid: u32,
    pub started: String,
}

/// Record the process `pid` (just started) by its start time.
pub(crate) fn record(table: &dyn ProcessTable, pid: Option<u32>) -> Option<Recorded> {
    let pid = pid?;
    let started = table.start_time(pid)?;
    Some(Recorded { pid, started })
}

/// Kill `recorded` if it is still the same process (same pid, same start time).
pub(crate) fn kill_if_same(table: &dyn ProcessTable, recorded: &Recorded) -> bool {
    if table.start_time(recorded.pid).as_deref() == Some(recorded.started.as_str()) {
        table.kill(recorded.pid);
        return true;
    }
    false
}
