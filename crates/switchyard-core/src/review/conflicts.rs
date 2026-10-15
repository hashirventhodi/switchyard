//! Conflicts in review: the hunks of files git holds unmerged (a merge or rebase stopped on
//! them; the working tree has conflict markers) and of files only a dry-run merge of base
//! conflicts on (#17's fact; the tree is untouched). Keep ours / Keep base resolve an
//! unmerged file's hunk by rewriting the file with that side: a plain file write, never
//! `git checkout --ours/--theirs`. Once no markers remain git is told the file is resolved.

use std::path::Path;

use switchyard_schema::command_name;
use switchyard_schema::{ConflictFile, ConflictHunk, ConflictKind, ConflictSide, Event, SessionStatus};

use super::diff::Fnv;
use crate::{git, sessions, Inner};

/// Lines of unconflicted context shown either side of a hunk.
const CONTEXT: usize = 3;

/// One conflict region of a file, by line index (lines keep their endings).
struct Region {
    /// The `<<<<<<<` line.
    start: usize,
    /// The `>>>>>>>` line.
    end: usize,
    first: Vec<String>,
    second: Vec<String>,
}

fn marker(line: &str, mark: &str) -> bool {
    let rest = line.strip_prefix(mark).map(|r| r.trim_end_matches(['\n', '\r']));
    rest.is_some_and(|r| r.is_empty() || r.starts_with(' '))
}

/// The file's conflict regions. A diff3-style ancestor section (`|||||||`) is dropped.
fn regions(lines: &[&str]) -> Vec<Region> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !marker(lines[i], "<<<<<<<") {
            i += 1;
            continue;
        }
        let start = i;
        let (mut first, mut second) = (Vec::new(), Vec::new());
        let mut part = 0; // 0 first side, 1 ancestor, 2 second side
        let mut end = None;
        i += 1;
        while i < lines.len() {
            let line = lines[i];
            if part < 2 && marker(line, "|||||||") {
                part = 1;
            } else if part < 2 && marker(line, "=======") {
                part = 2;
            } else if part == 2 && marker(line, ">>>>>>>") {
                end = Some(i);
                break;
            } else if part == 0 {
                first.push(line.to_string());
            } else if part == 2 {
                second.push(line.to_string());
            }
            i += 1;
        }
        // An unterminated region is not a conflict git wrote; stop looking.
        let Some(end) = end else { break };
        out.push(Region { start, end, first, second });
        i = end + 1;
    }
    out
}

fn bare(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| l.trim_end_matches(['\n', '\r']).to_string()).collect()
}

/// The branch's lines and base's lines of a region.
fn sides(region: &Region, base_first: bool) -> (&[String], &[String]) {
    if base_first {
        (&region.second, &region.first)
    } else {
        (&region.first, &region.second)
    }
}

/// `nth`: how many earlier regions of the file have the same two sides, so resolving one
/// conflict leaves the others' ids alone.
fn hunk_id(path: &str, nth: usize, ours: &[String], base: &[String]) -> String {
    let mut h = Fnv::new();
    h.add(format!("{path}\0{nth}\0").as_bytes());
    for l in ours {
        h.add(b"<");
        h.add(l.as_bytes());
    }
    for l in base {
        h.add(b">");
        h.add(l.as_bytes());
    }
    format!("{:016x}", h.0)
}

/// Each region's id, in order.
fn ids(path: &str, found: &[Region], base_first: bool) -> Vec<String> {
    found
        .iter()
        .enumerate()
        .map(|(n, r)| {
            let nth = found[..n].iter().filter(|e| e.first == r.first && e.second == r.second).count();
            let (ours, base) = sides(r, base_first);
            hunk_id(path, nth, ours, base)
        })
        .collect()
}

fn hunks(path: &str, text: &str, base_first: bool) -> Vec<ConflictHunk> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let found = regions(&lines);
    let own = |a: usize, b: usize| bare(&lines[a..b].iter().map(|l| l.to_string()).collect::<Vec<_>>());
    let ids = ids(path, &found, base_first);
    found
        .iter()
        .enumerate()
        .map(|(n, r)| {
            let floor = if n == 0 { 0 } else { found[n - 1].end + 1 };
            let ceiling = found.get(n + 1).map_or(lines.len(), |next| next.start);
            let (ours, base) = sides(r, base_first);
            ConflictHunk {
                id: ids[n].clone(),
                before: own(r.start.saturating_sub(CONTEXT).max(floor), r.start),
                ours: bare(ours),
                base: bare(base),
                after: own(r.end + 1, (r.end + 1 + CONTEXT).min(ceiling)),
            }
        })
        .collect()
}

/// Every conflicting file of the worktree with its hunks: unmerged files first, then files
/// a merge of base would conflict on (when behind). Also returns base's name.
pub(crate) fn read(inner: &Inner, worktree_path: &str) -> (String, Vec<ConflictFile>) {
    let dir = Path::new(worktree_path);
    let mut files = Vec::new();
    let unmerged = git::unmerged_paths(dir).unwrap_or_default();
    let base_first = !unmerged.is_empty() && git::rebase_in_progress(dir);
    for path in &unmerged {
        let text = std::fs::read(dir.join(path)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        files.push(ConflictFile { path: path.clone(), kind: ConflictKind::Unmerged, hunks: hunks(path, &text, base_first) });
    }
    let (base, base_ref, behind) = super::base_of(inner, worktree_path);
    if behind > 0 {
        if let Ok(Some((tree, paths))) = git::dry_run_merge(dir, &base_ref) {
            for path in paths.into_iter().filter(|p| !unmerged.contains(p)) {
                let text = git::blob_text(dir, &tree, &path).unwrap_or_default();
                let hunks = hunks(&path, &text, false);
                files.push(ConflictFile { path, kind: ConflictKind::Base, hunks });
            }
        }
    }
    (base, files)
}

pub(crate) fn resolve(inner: &mut Inner, worktree_path: &str, path: &str, hunk_id: &str, side: ConflictSide) {
    const COMMAND: &str = command_name::RESOLVE_CONFLICT;
    if inner.find_worktree(worktree_path).is_none() {
        return inner.refuse(COMMAND, format!("{worktree_path} is not a worktree of an added repository"));
    }
    // Switchyard never rewrites the tree under a working agent.
    if sessions::occupant(inner, worktree_path) == Some(SessionStatus::Running) {
        return inner.refuse(
            COMMAND,
            "This worktree has a Running session; stop it or ask the agent to resolve the conflict".into(),
        );
    }
    if sessions::awaits_request(inner, worktree_path) {
        return inner.refuse(COMMAND, "The agent is waiting on a request mid-turn; answer its request first".into());
    }
    let dir = Path::new(worktree_path);
    let unmerged = git::unmerged_paths(dir).unwrap_or_default();
    if !unmerged.iter().any(|p| p == path) {
        let (base, conflicts) = super::base_conflicts(inner, worktree_path);
        let reason = if conflicts.iter().any(|c| c == path) {
            format!("{path} conflicts with {base} only in a merge of {base} that has not happened; rebase, or ask the agent to rebase")
        } else {
            format!("{path} has no conflict to resolve")
        };
        return inner.refuse(COMMAND, reason);
    }
    let file = dir.join(path);
    let text = match std::fs::read(&file) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => return inner.refuse(COMMAND, format!("Could not read {path}: {e}")),
    };
    let base_first = git::rebase_in_progress(dir);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let found = regions(&lines);
    let at = ids(path, &found, base_first).iter().position(|id| id == hunk_id);
    let Some(at) = at else {
        return inner.refuse(COMMAND, format!("That conflict in {path} has changed; read the changes again"));
    };
    let region = &found[at];
    let (ours, base) = sides(region, base_first);
    let kept = match side {
        ConflictSide::Ours => ours,
        ConflictSide::Base => base,
    };
    let mut out = String::with_capacity(text.len());
    lines[..region.start].iter().for_each(|l| out.push_str(l));
    kept.iter().for_each(|l| out.push_str(l));
    // The last kept line may lack the newline the `=======` / `>>>>>>>` line supplied.
    if !out.is_empty() && !out.ends_with('\n') && region.end + 1 < lines.len() {
        out.push('\n');
    }
    lines[region.end + 1..].iter().for_each(|l| out.push_str(l));
    if let Err(e) = std::fs::write(&file, &out) {
        return inner.refuse(COMMAND, format!("Could not write {path}: {e}"));
    }
    let remaining = (found.len() - 1) as u32;
    if remaining == 0 {
        if let Err(git::GitError(e)) = git::mark_resolved(dir, path) {
            return inner.refuse(COMMAND, format!("{path} has no conflict markers left, but git could not mark it resolved: {e}"));
        }
    }
    inner.emit(Event::ConflictResolved { worktree_path: worktree_path.into(), path: path.into(), side, remaining });
    super::emit_changes(inner, COMMAND, worktree_path);
}
