//! A worktree's context as its agents see it: the instruction files in the worktree
//! (CLAUDE.md, AGENTS.md and their variants) and the memory Claude Code keeps for the
//! worktree's directory. Read-only: nothing here writes.

use std::path::{Path, PathBuf};

use switchyard_schema::command_name;
use switchyard_schema::{ContextFile, ContextKind, Event};

use crate::Inner;

/// Instruction files, relative to the worktree, in the order they are shown.
const INSTRUCTIONS: &[&str] =
    &["CLAUDE.md", "CLAUDE.local.md", ".claude/CLAUDE.md", "AGENTS.md", "AGENTS.override.md", ".codex/AGENTS.md"];

/// The most of one file sent to the renderer.
pub(crate) const MAX_BYTES: usize = 64 * 1024;

/// `~/.claude`, where Claude Code keeps per-project memory.
pub fn default_claude_home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join(".claude")
}

/// Claude Code's memory directory for a session started in `dir`.
fn memory_dir(claude_home: &Path, dir: &str) -> PathBuf {
    let slug: String = dir.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    claude_home.join("projects").join(slug).join("memory")
}

fn read(name: String, path: &Path, kind: ContextKind) -> Option<ContextFile> {
    if !path.is_file() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let truncated = bytes.len() > MAX_BYTES;
    let mut end = bytes.len().min(MAX_BYTES);
    let text = loop {
        match std::str::from_utf8(&bytes[..end]) {
            Ok(t) => break t.to_string(),
            // Cut mid-character: back off to the last whole one.
            Err(e) if truncated && e.valid_up_to() > 0 && end - e.valid_up_to() < 4 => end = e.valid_up_to(),
            Err(_) => break String::from_utf8_lossy(&bytes[..end]).into_owned(),
        }
    };
    Some(ContextFile { name, path: path.to_string_lossy().into_owned(), kind, text, truncated })
}

/// [`Command::ReadContext`](switchyard_schema::Command::ReadContext).
pub(crate) fn read_context(inner: &Inner, worktree_path: &str) {
    const COMMAND: &str = command_name::READ_CONTEXT;
    let Some((_, worktree)) = inner.find_worktree(worktree_path) else {
        return inner.refuse(COMMAND, format!("{worktree_path} is not a worktree of an added repository"));
    };
    let mut files: Vec<ContextFile> = INSTRUCTIONS
        .iter()
        .filter_map(|name| read(name.to_string(), &worktree.path.join(name), ContextKind::Instructions))
        .collect();
    let memory = memory_dir(&inner.runtime.claude_home, worktree_path);
    let mut notes: Vec<PathBuf> = std::fs::read_dir(&memory)
        .map(|list| list.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    // The index first, then the notes it points at by name.
    notes.sort_by_key(|p| (p.file_name().is_none_or(|n| n != "MEMORY.md"), p.clone()));
    for note in notes.into_iter().filter(|p| p.extension().is_some_and(|e| e == "md")) {
        let name = note.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        files.extend(read(name, &note, ContextKind::Memory));
    }
    inner.emit(Event::ContextRead { worktree_path: worktree_path.into(), files });
}
