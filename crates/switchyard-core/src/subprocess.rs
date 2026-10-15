//! Running a CLI that talks to the network (`gh`, `git push`) with a time limit: a hung
//! connection or a credential prompt nobody can answer must not hang the core.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Run `cmd` to completion, or kill it (and its process group) after `limit`.
/// `Ok(None)`: it did not finish in time.
pub(crate) fn output_within(cmd: &mut Command, limit: Duration) -> std::io::Result<Option<Output>> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            // The whole group: `git push` runs `ssh`, which would keep the pipes open.
            let _ = Command::new("kill").args(["-KILL", &format!("-{}", child.id())]).status();
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok(Some(Output { status, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() }))
}

/// "1 minute", "30 seconds", "300 ms": how long was waited, for the user.
pub(crate) fn describe(limit: Duration) -> String {
    let secs = limit.as_secs();
    if secs == 0 {
        format!("{} ms", limit.as_millis())
    } else if secs % 60 == 0 {
        let m = secs / 60;
        if m == 1 { "1 minute".into() } else { format!("{m} minutes") }
    } else if secs == 1 {
        "1 second".into()
    } else {
        format!("{secs} seconds")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_command_answers() {
        let out = output_within(Command::new("sh").args(["-c", "echo hi; echo oops >&2"]), Duration::from_secs(10))
            .unwrap()
            .expect("finished");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hi\n");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "oops\n");
    }

    #[test]
    fn a_hung_command_and_its_children_are_killed_at_the_limit() {
        let started = Instant::now();
        let out = output_within(Command::new("sh").args(["-c", "sleep 30 & sleep 30"]), Duration::from_millis(200)).unwrap();
        assert!(out.is_none());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn limits_read_as_words() {
        assert_eq!(describe(Duration::from_secs(60)), "1 minute");
        assert_eq!(describe(Duration::from_secs(120)), "2 minutes");
        assert_eq!(describe(Duration::from_secs(30)), "30 seconds");
        assert_eq!(describe(Duration::from_millis(300)), "300 ms");
    }
}
