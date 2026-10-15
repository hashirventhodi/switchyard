//! Parses `git diff` patch output (worktree against the index) into changed files and
//! hunks with stable ids.

use switchyard_schema::{ChangedFile, DiffLine, FileStatus, Hunk, LineKind};

/// One parsed file plus what its review decision is pinned to.
pub(crate) struct Parsed {
    pub file: ChangedFile,
    /// Changes whenever the file's diff does: a decision taken on another fingerprint no
    /// longer applies.
    pub fingerprint: u64,
}

#[derive(Default)]
struct Building {
    path: Option<String>,
    old_path: Option<String>,
    header_path: Option<String>,
    status: Option<FileStatus>,
    binary: bool,
    index_line: String,
    hunks: Vec<Hunk>,
    added: u32,
    removed: u32,
    /// Lines of the current hunk still expected on each side.
    old_left: u32,
    new_left: u32,
    old_no: u32,
    new_no: u32,
    hunk_start: String,
    conflict: bool,
}

pub(crate) fn parse(patch: &str) -> Vec<Parsed> {
    let mut out = Vec::new();
    let mut cur: Option<Building> = None;
    for line in patch.lines() {
        let in_hunk = cur.as_ref().is_some_and(|b| b.old_left > 0 || b.new_left > 0);
        if in_hunk {
            let b = cur.as_mut().expect("in a file");
            hunk_line(b, line);
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            finish(&mut cur, &mut out);
            cur = Some(Building { header_path: header_path(rest), ..Default::default() });
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --cc ").or_else(|| line.strip_prefix("diff --combined ")) {
            finish(&mut cur, &mut out);
            cur = Some(Building { path: Some(unquote(rest)), conflict: true, ..Default::default() });
            continue;
        }
        let Some(b) = cur.as_mut() else { continue };
        if b.conflict {
            continue;
        }
        if line.starts_with("new file mode") {
            b.status = Some(FileStatus::Added);
        } else if line.starts_with("deleted file mode") {
            b.status = Some(FileStatus::Deleted);
        } else if let Some(p) = line.strip_prefix("rename from ") {
            b.old_path = Some(unquote(p));
            b.status = Some(FileStatus::Renamed);
        } else if let Some(p) = line.strip_prefix("rename to ") {
            b.path = Some(unquote(p));
        } else if let Some(p) = line.strip_prefix("--- ") {
            if p != "/dev/null" && b.path.is_none() {
                b.path = Some(strip_side(&unquote(p), "a/"));
            }
        } else if let Some(p) = line.strip_prefix("+++ ") {
            if p != "/dev/null" {
                b.path = Some(strip_side(&unquote(p), "b/"));
            }
        } else if line.starts_with("index ") {
            b.index_line = line.to_string();
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            b.binary = true;
        } else if line.starts_with("@@ ") {
            start_hunk(b, line);
        }
    }
    finish(&mut cur, &mut out);
    out
}

fn start_hunk(b: &mut Building, line: &str) {
    // @@ -a[,b] +c[,d] @@ context
    let mut parts = line.split(' ').skip(1);
    let old = parts.next().unwrap_or("-0").trim_start_matches('-');
    let new = parts.next().unwrap_or("+0").trim_start_matches('+');
    let range = |r: &str| {
        let mut it = r.splitn(2, ',');
        let start = it.next().and_then(|n| n.parse().ok()).unwrap_or(0u32);
        let len = it.next().map_or(1, |n| n.parse().unwrap_or(0));
        (start, len)
    };
    let (os, ol) = range(old);
    let (ns, nl) = range(new);
    b.old_no = os;
    b.new_no = ns;
    b.old_left = ol;
    b.new_left = nl;
    b.hunk_start = format!("-{old}");
    b.hunks.push(Hunk {
        id: String::new(),
        header: line.to_string(),
        lines: Vec::new(),
        decision: None,
        comment: None,
        sent: false,
    });
}

fn hunk_line(b: &mut Building, line: &str) {
    let (kind, text) = match line.as_bytes().first() {
        Some(b'+') => (LineKind::Added, &line[1..]),
        Some(b'-') => (LineKind::Removed, &line[1..]),
        Some(b' ') => (LineKind::Context, &line[1..]),
        Some(b'\\') => return, // "\ No newline at end of file"
        None => (LineKind::Context, ""),
        _ => (LineKind::Context, line),
    };
    let (old, new) = match kind {
        LineKind::Context => {
            b.old_left = b.old_left.saturating_sub(1);
            b.new_left = b.new_left.saturating_sub(1);
            (Some(b.old_no), Some(b.new_no))
        }
        LineKind::Removed => {
            b.old_left = b.old_left.saturating_sub(1);
            b.removed += 1;
            (Some(b.old_no), None)
        }
        LineKind::Added => {
            b.new_left = b.new_left.saturating_sub(1);
            b.added += 1;
            (None, Some(b.new_no))
        }
    };
    if old.is_some() {
        b.old_no += 1;
    }
    if new.is_some() {
        b.new_no += 1;
    }
    let hunk = b.hunks.last_mut().expect("a hunk started");
    hunk.lines.push(DiffLine { kind, old, new, text: text.to_string() });
}

fn finish(cur: &mut Option<Building>, out: &mut Vec<Parsed>) {
    let Some(mut b) = cur.take() else { return };
    let Some(path) = b.path.take().or(b.header_path.take()) else { return };
    let status = if b.conflict { FileStatus::Conflicted } else { b.status.unwrap_or(FileStatus::Modified) };
    let id_path = b.old_path.clone().unwrap_or_else(|| path.clone());
    let mut fp = Fnv::new();
    fp.add(format!("{status:?}\0{path}\0{id_path}\0{}", b.index_line).as_bytes());
    for hunk in &mut b.hunks {
        hunk.id = hunk_id(&id_path, &hunk.header, &hunk.lines);
        fp.add(hunk.id.as_bytes());
    }
    out.push(Parsed {
        fingerprint: fp.0,
        file: ChangedFile {
            path,
            old_path: b.old_path,
            status,
            added: b.added,
            removed: b.removed,
            binary: b.binary,
            decision: None,
            hunks: b.hunks,
        },
    });
}

/// Hash of the index-side path, the index-side range and the hunk's lines: unchanged when
/// the working-tree side shifts (an edit above it) or the file is renamed.
fn hunk_id(path: &str, header: &str, lines: &[DiffLine]) -> String {
    let old_range = header.split(' ').nth(1).unwrap_or("");
    let mut h = Fnv::new();
    h.add(path.as_bytes());
    h.add(b"\0");
    h.add(old_range.as_bytes());
    for l in lines {
        h.add(match l.kind {
            LineKind::Context => b" ",
            LineKind::Added => b"+",
            LineKind::Removed => b"-",
        });
        h.add(l.text.as_bytes());
        h.add(b"\n");
    }
    format!("{:016x}", h.0)
}

/// `a/x b/x` from a `diff --git` line, when both sides name the same path.
fn header_path(rest: &str) -> Option<String> {
    if rest.starts_with('"') {
        return None;
    }
    let half = rest.len().checked_sub(1)? / 2;
    let (a, b) = (rest.get(..half)?, rest.get(half + 1..)?);
    let (a, b) = (a.strip_prefix("a/")?, b.strip_prefix("b/")?);
    (a == b).then(|| a.to_string())
}

fn strip_side(p: &str, side: &str) -> String {
    p.strip_prefix(side).unwrap_or(p).to_string()
}

/// Undo git's C-style quoting of unusual paths.
fn unquote(p: &str) -> String {
    let Some(inner) = p.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else { return p.to_string() };
    let mut bytes = Vec::new();
    let mut it = inner.bytes().peekable();
    while let Some(c) = it.next() {
        if c != b'\\' {
            bytes.push(c);
            continue;
        }
        match it.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'"') => bytes.push(b'"'),
            Some(b'\\') => bytes.push(b'\\'),
            Some(d @ b'0'..=b'7') => {
                let mut v = u32::from(d - b'0');
                for _ in 0..2 {
                    if let Some(&n @ b'0'..=b'7') = it.peek() {
                        v = v * 8 + u32::from(n - b'0');
                        it.next();
                    }
                }
                bytes.push(v as u8);
            }
            Some(other) => bytes.push(other),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// 64-bit FNV-1a: stable across builds and platforms.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn add(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}
