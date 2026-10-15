//! Shell terminals: a PTY running the user's shell in a worktree, for the dock. Shells belong
//! to a worktree, never to a Session (GLOSSARY: Terminal).
//!
//! Each shell has two threads. The writer owns the PTY's input side and drains a channel, so
//! a command never blocks the core on a full PTY buffer. The reader owns the child and the
//! PTY's output side; it re-enters the core (through a `Weak`, like agent output) with each
//! chunk of output and, when the shell ends, with its exit code.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, Weak};

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use switchyard_schema::{Event, Shell};

use crate::processes::{self, Recorded};
use crate::session_store;
use crate::Inner;

/// Recent output kept per shell, replayed on attach so a reloaded renderer keeps its screen.
const SCROLLBACK_BYTES: usize = 256 * 1024;

pub(crate) struct Shells {
    list: Vec<ShellState>,
    next_id: u64,
    program: ShellProgram,
}

/// What runs in a shell terminal.
#[derive(Debug, Clone)]
pub(crate) struct ShellProgram {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl Default for ShellProgram {
    /// The user's login shell, as their terminal would start it.
    fn default() -> Self {
        let program = std::env::var_os("SHELL").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/bin/zsh"));
        ShellProgram { program, args: vec!["-l".into()] }
    }
}

impl Default for Shells {
    fn default() -> Self {
        Shells { list: Vec::new(), next_id: 0, program: ShellProgram::default() }
    }
}

impl Drop for Shells {
    fn drop(&mut self) {
        for state in &mut self.list {
            let _ = state.killer.kill();
        }
    }
}

impl Shells {
    pub(crate) fn set_program(&mut self, program: ShellProgram) {
        self.program = program;
    }
}

struct ShellState {
    shell: Shell,
    master: Box<dyn MasterPty + Send>,
    input: Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    scrollback: String,
    /// The shell process, recorded in the store so a relaunch after a crash kills it.
    process: Option<Recorded>,
}

pub(crate) fn open(inner: &mut Inner, core: &Arc<Mutex<Inner>>, worktree_path: &str, cols: u16, rows: u16) {
    const COMMAND: &str = "openShell";
    let Some((_, worktree)) = inner.find_worktree(worktree_path) else {
        return inner.refuse(COMMAND, format!("{worktree_path} is not a worktree of an added repository"));
    };
    let id = format!("shell-{}", inner.shells.next_id + 1);
    let spawned = spawn(&inner.shells.program, &worktree.path, size(cols, rows));
    let (master, child, reader, writer) = match spawned {
        Ok(parts) => parts,
        Err(e) => return inner.refuse(COMMAND, format!("Could not start a shell: {e}")),
    };
    inner.shells.next_id += 1;
    let killer = child.clone_killer();
    let process = processes::record(&*inner.runtime.processes, child.process_id());
    if let Some(process) = &process {
        let _ = session_store::record_process(inner.store.db(), process, &format!("shell {id} in {worktree_path}"));
    }

    let (input, inbox) = channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut writer = writer;
        for bytes in inbox {
            if writer.write_all(&bytes).and_then(|_| writer.flush()).is_err() {
                break;
            }
        }
    });

    let weak = Arc::downgrade(core);
    let reader_id = id.clone();
    std::thread::spawn(move || read_until_exit(weak, reader_id, reader, child));

    let shell = Shell { id, worktree_path: worktree_path.into() };
    inner.emit(Event::ShellOpened { shell: shell.clone() });
    inner.shells.list.push(ShellState { shell, master, input, killer, scrollback: String::new(), process });
}

pub(crate) fn write(inner: &mut Inner, id: &str, data: &str) {
    let Some(state) = inner.shells.list.iter().find(|s| s.shell.id == id) else {
        return inner.refuse("writeShell", format!("There is no shell {id}"));
    };
    let _ = state.input.send(data.as_bytes().to_vec());
}

pub(crate) fn resize(inner: &mut Inner, id: &str, cols: u16, rows: u16) {
    let Some(state) = inner.shells.list.iter().find(|s| s.shell.id == id) else {
        return inner.refuse("resizeShell", format!("There is no shell {id}"));
    };
    if let Err(e) = state.master.resize(size(cols, rows)) {
        inner.refuse("resizeShell", format!("Could not resize the shell: {e}"));
    }
}

pub(crate) fn close(inner: &mut Inner, id: &str) {
    if !inner.shells.list.iter().any(|s| s.shell.id == id) {
        return inner.refuse("closeShell", format!("There is no shell {id}"));
    }
    kill_where(inner, |s| s.id == id);
}

/// Close every shell on a worktree. The worktree-removal path (Merged cleanup, prune) calls
/// this before the worktree goes; the `closeWorktreeShells` command does the same.
pub(crate) fn close_worktree(inner: &mut Inner, worktree_path: &str) {
    kill_where(inner, |s| s.worktree_path == worktree_path);
}

/// Emit every open shell and its recent output (on attach).
pub(crate) fn replay(inner: &Inner) {
    for state in &inner.shells.list {
        inner.emit(Event::ShellOpened { shell: state.shell.clone() });
        if !state.scrollback.is_empty() {
            inner.emit(Event::ShellOutput { shell_id: state.shell.id.clone(), data: state.scrollback.clone() });
        }
    }
}

fn kill_where(inner: &mut Inner, matches: impl Fn(&Shell) -> bool) {
    let (closing, kept) = std::mem::take(&mut inner.shells.list).into_iter().partition(|s| matches(&s.shell));
    inner.shells.list = kept;
    for mut state in closing {
        let _ = state.killer.kill();
        forget(inner, &state);
        // The reader thread finds the shell gone and stays quiet: this is the one close event.
        inner.emit(Event::ShellClosed { shell_id: state.shell.id.clone(), exit_code: None });
    }
}

/// The shell is gone: nothing to kill after a crash.
fn forget(inner: &Inner, state: &ShellState) {
    if let Some(process) = &state.process {
        let _ = session_store::forget_process(inner.store.db(), process.pid);
    }
}

/// On open: kill every shell a previous run left behind (same pid, same start time). Shells
/// are never adopted; the dock starts empty.
pub(crate) fn kill_orphans(inner: &Inner) {
    let Ok(recorded) = session_store::take_processes(inner.store.db()) else { return };
    for process in recorded {
        processes::kill_if_same(&*inner.runtime.processes, &process);
    }
}

type Spawned = (
    Box<dyn MasterPty + Send>,
    Box<dyn portable_pty::Child + Send + Sync>,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
);

fn spawn(program: &ShellProgram, cwd: &std::path::Path, size: PtySize) -> Result<Spawned, String> {
    let pair = native_pty_system().openpty(size).map_err(|e| e.to_string())?;
    let mut cmd = CommandBuilder::new(&program.program);
    cmd.args(&program.args);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    // Only the child may hold the PTY's terminal side, or the reader never sees it end.
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    Ok((pair.master, child, reader, writer))
}

fn read_until_exit(
    core: Weak<Mutex<Inner>>,
    id: String,
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) {
    let mut buf = [0u8; 8192];
    let mut pending = Vec::new();
    // Once the shell is closed (or the core gone) keep draining until EOF without emitting:
    // a hung-up shell can block in exit flushing its terminal if nobody reads the other end.
    let mut closed = false;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if closed {
            continue;
        }
        let text = decode_utf8(&mut pending, &buf[..n]);
        if text.is_empty() {
            continue;
        }
        let Some(core) = core.upgrade() else {
            closed = true;
            continue;
        };
        let mut inner = core.lock().expect("core lock");
        let Some(state) = inner.shells.list.iter_mut().find(|s| s.shell.id == id) else {
            closed = true;
            continue;
        };
        push_scrollback(&mut state.scrollback, &text);
        inner.emit(Event::ShellOutput { shell_id: id.clone(), data: text });
    }
    let exit_code = child.wait().ok().map(|status| status.exit_code());
    let Some(core) = core.upgrade() else { return };
    let mut inner = core.lock().expect("core lock");
    if let Some(at) = inner.shells.list.iter().position(|s| s.shell.id == id) {
        let state = inner.shells.list.remove(at);
        forget(&inner, &state);
        inner.emit(Event::ShellClosed { shell_id: id, exit_code });
    }
}

/// Decode `bytes` after any incomplete character left in `pending` by the previous read;
/// keeps a trailing incomplete character for the next one. Invalid bytes become U+FFFD.
fn decode_utf8(pending: &mut Vec<u8>, bytes: &[u8]) -> String {
    pending.extend_from_slice(bytes);
    let mut out = String::new();
    let mut rest: &[u8] = pending;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                rest = &[];
                break;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(valid).expect("checked"));
                match e.error_len() {
                    Some(bad) => {
                        out.push('\u{FFFD}');
                        rest = &after[bad..];
                    }
                    None => {
                        rest = after;
                        break;
                    }
                }
            }
        }
    }
    *pending = rest.to_vec();
    out
}

fn push_scrollback(scrollback: &mut String, text: &str) {
    scrollback.push_str(text);
    if scrollback.len() > SCROLLBACK_BYTES {
        let mut cut = scrollback.len() - SCROLLBACK_BYTES;
        while !scrollback.is_char_boundary(cut) {
            cut += 1;
        }
        scrollback.drain(..cut);
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize { cols: cols.max(1), rows: rows.max(1), pixel_width: 0, pixel_height: 0 }
}
