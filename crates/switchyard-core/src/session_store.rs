//! Sessions in the SQLite store: one row per Session (live or archived), its transcript
//! as deflate-compressed chunks of [`CHUNK`] entries (a replaced entry rewrites its chunk),
//! its raw token usage (never estimates), the user settings, review decisions and hunk
//! reviews (until commit; `review` keeps them in memory too), and the processes other than
//! agents (shell PTYs) the core started, by pid and start time.

use std::io::Read;

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use adapter_protocol::PermissionMode as ProtocolMode;
use rusqlite::{params, Connection, OptionalExtension};
use switchyard_schema::{Entry, Session, SessionFacts, Settings};

use crate::processes::Recorded;
use crate::spend::Usage;

/// Transcript entries per stored chunk.
pub(crate) const CHUNK: usize = 64;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id             TEXT PRIMARY KEY,
    seq            INTEGER NOT NULL,
    repository_id  TEXT NOT NULL,
    branch         TEXT,
    session        TEXT NOT NULL,
    facts          TEXT NOT NULL,
    expected_mode  TEXT,
    external_id    TEXT,
    pid            INTEGER,
    pid_started    TEXT,
    entries        INTEGER NOT NULL DEFAULT 0,
    archived_at_ms INTEGER
);
CREATE INDEX IF NOT EXISTS sessions_archive ON sessions (repository_id, branch, archived_at_ms);
CREATE TABLE IF NOT EXISTS transcript_chunks (
    session_id TEXT NOT NULL,
    chunk      INTEGER NOT NULL,
    data       BLOB NOT NULL,
    PRIMARY KEY (session_id, chunk)
);
CREATE TABLE IF NOT EXISTS session_usage (
    session_id        TEXT NOT NULL,
    seq               INTEGER NOT NULL,
    at_ms             INTEGER NOT NULL,
    input_tokens      INTEGER NOT NULL,
    output_tokens     INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL,
    reported_usd      REAL,
    PRIMARY KEY (session_id, seq)
);
CREATE TABLE IF NOT EXISTS processes (
    pid     INTEGER PRIMARY KEY,
    started TEXT NOT NULL,
    what    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS review_decisions (
    worktree_path TEXT NOT NULL,
    path          TEXT NOT NULL,
    decision      TEXT NOT NULL,
    fingerprint   INTEGER NOT NULL,
    PRIMARY KEY (worktree_path, path)
);
CREATE TABLE IF NOT EXISTS review_hunks (
    worktree_path TEXT NOT NULL,
    hunk_id       TEXT NOT NULL,
    decision      TEXT,
    comment       TEXT,
    sent          INTEGER NOT NULL,
    PRIMARY KEY (worktree_path, hunk_id)
);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

pub(crate) fn init(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(SCHEMA)?;
    // Stores made before sessions kept the mode asked of the CLI.
    let has_expected_mode = db
        .prepare("SELECT 1 FROM pragma_table_info('sessions') WHERE name = 'expected_mode'")?
        .exists([])?;
    if !has_expected_mode {
        db.execute_batch("ALTER TABLE sessions ADD COLUMN expected_mode TEXT")?;
    }
    Ok(())
}

/// What is stored about a Session besides its transcript and usage.
pub(crate) struct Row {
    pub session: Session,
    pub facts: SessionFacts,
    /// The mode asked of the CLI, in protocol terms; `None` in rows stored before it was kept.
    pub expected_mode: Option<ProtocolMode>,
    pub external_id: Option<String>,
    pub process: Option<Recorded>,
    pub entries: usize,
    pub archived_at_ms: Option<u64>,
}

fn seq(id: &str) -> i64 {
    id.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0)
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("schema types serialize")
}

fn bad(e: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(e.to_string().into())
}

pub(crate) fn save_row(db: &Connection, row: &Row) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO sessions (id, seq, repository_id, branch, session, facts, external_id, pid, pid_started, entries, archived_at_ms, expected_mode)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET session = ?5, facts = ?6, external_id = ?7, pid = ?8, pid_started = ?9,
             entries = ?10, archived_at_ms = ?11, expected_mode = ?12",
        params![
            row.session.id,
            seq(&row.session.id),
            row.session.repository_id,
            row.session.branch,
            json(&row.session),
            json(&row.facts),
            row.external_id,
            row.process.as_ref().map(|p| p.pid),
            row.process.as_ref().map(|p| p.started.clone()),
            row.entries as i64,
            row.archived_at_ms.map(|t| t as i64),
            row.expected_mode.as_ref().map(json),
        ],
    )?;
    Ok(())
}

/// Store transcript chunk number `chunk` (its entries, in order).
pub(crate) fn save_chunk(db: &Connection, session_id: &str, chunk: usize, entries: &[Entry]) -> rusqlite::Result<()> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
    serde_json::to_writer(&mut encoder, entries).map_err(bad)?;
    let data = encoder.finish().map_err(bad)?;
    db.execute(
        "INSERT INTO transcript_chunks (session_id, chunk, data) VALUES (?1, ?2, ?3)
         ON CONFLICT (session_id, chunk) DO UPDATE SET data = ?3",
        params![session_id, chunk as i64, data],
    )?;
    Ok(())
}

pub(crate) fn save_usage(db: &Connection, session_id: &str, seq: usize, usage: &Usage) -> rusqlite::Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO session_usage
         (session_id, seq, at_ms, input_tokens, output_tokens, cache_read_tokens, reported_usd)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            session_id,
            seq as i64,
            usage.at_ms as i64,
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cache_read_tokens as i64,
            usage.reported_usd,
        ],
    )?;
    Ok(())
}

/// Every stored Session, oldest first (live and archived alike).
pub(crate) fn rows(db: &Connection) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = db.prepare(
        "SELECT session, facts, external_id, pid, pid_started, entries, archived_at_ms, expected_mode FROM sessions ORDER BY seq",
    )?;
    let rows = stmt.query_map([], |r| {
        let session: String = r.get(0)?;
        let facts: String = r.get(1)?;
        let pid: Option<i64> = r.get(3)?;
        let started: Option<String> = r.get(4)?;
        let archived: Option<i64> = r.get(6)?;
        let expected_mode: Option<String> = r.get(7)?;
        Ok(Row {
            session: serde_json::from_str(&session).map_err(bad)?,
            facts: serde_json::from_str(&facts).map_err(bad)?,
            expected_mode: expected_mode.map(|m| serde_json::from_str(&m)).transpose().map_err(bad)?,
            external_id: r.get(2)?,
            process: pid.zip(started).map(|(pid, started)| Recorded { pid: pid as u32, started }),
            entries: r.get::<_, i64>(5)? as usize,
            archived_at_ms: archived.map(|t| t as u64),
        })
    })?;
    rows.collect()
}

pub(crate) fn transcript(db: &Connection, session_id: &str) -> rusqlite::Result<Vec<Entry>> {
    let mut stmt = db.prepare("SELECT data FROM transcript_chunks WHERE session_id = ?1 ORDER BY chunk")?;
    let chunks = stmt.query_map(params![session_id], |r| r.get::<_, Vec<u8>>(0))?;
    let mut entries = Vec::new();
    let mut text = Vec::new();
    for data in chunks {
        text.clear();
        DeflateDecoder::new(&data?[..]).read_to_end(&mut text).map_err(bad)?;
        let chunk: Vec<Entry> = serde_json::from_slice(&text).map_err(bad)?;
        entries.extend(chunk);
    }
    Ok(entries)
}

pub(crate) fn usage(db: &Connection, session_id: &str) -> rusqlite::Result<Vec<Usage>> {
    let mut stmt = db.prepare(
        "SELECT at_ms, input_tokens, output_tokens, cache_read_tokens, reported_usd
         FROM session_usage WHERE session_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![session_id], |r| {
        Ok(Usage {
            at_ms: r.get::<_, i64>(0)? as u64,
            input_tokens: r.get::<_, i64>(1)? as u64,
            output_tokens: r.get::<_, i64>(2)? as u64,
            cache_read_tokens: r.get::<_, i64>(3)? as u64,
            reported_usd: r.get(4)?,
        })
    })?;
    rows.collect()
}

/// Forget an archived Session for good.
pub(crate) fn delete(db: &Connection, session_id: &str) -> rusqlite::Result<()> {
    db.execute("DELETE FROM transcript_chunks WHERE session_id = ?1", params![session_id])?;
    db.execute("DELETE FROM session_usage WHERE session_id = ?1", params![session_id])?;
    db.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    Ok(())
}

pub(crate) fn settings(db: &Connection) -> Settings {
    db.query_row("SELECT value FROM settings WHERE key = 'settings'", [], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(crate) fn save_settings(db: &Connection, settings: &Settings) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO settings (key, value) VALUES ('settings', ?1) ON CONFLICT (key) DO UPDATE SET value = ?1",
        params![json(settings)],
    )?;
    Ok(())
}

/// Write `f`'s changes in one transaction.
pub(crate) fn batch(db: &Connection, f: impl FnOnce(&Connection) -> rusqlite::Result<()>) -> rusqlite::Result<()> {
    db.execute_batch("BEGIN")?;
    match f(db) {
        Ok(()) => db.execute_batch("COMMIT"),
        Err(e) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}


/// Remember a process the core started (`what` says which, for diagnosis).
pub(crate) fn record_process(db: &Connection, process: &Recorded, what: &str) -> rusqlite::Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO processes (pid, started, what) VALUES (?1, ?2, ?3)",
        params![process.pid, process.started, what],
    )?;
    Ok(())
}

pub(crate) fn forget_process(db: &Connection, pid: u32) -> rusqlite::Result<()> {
    db.execute("DELETE FROM processes WHERE pid = ?1", params![pid])?;
    Ok(())
}

/// Every recorded process, forgetting them all (the caller deals with them now).
pub(crate) fn take_processes(db: &Connection) -> rusqlite::Result<Vec<Recorded>> {
    let list = {
        let mut stmt = db.prepare("SELECT pid, started FROM processes")?;
        let rows = stmt.query_map([], |r| Ok(Recorded { pid: r.get::<_, i64>(0)? as u32, started: r.get(1)? }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    db.execute("DELETE FROM processes", [])?;
    Ok(list)
}

/// One worktree's review decisions: path -> (decision, fingerprint of the diff it was taken on).
pub(crate) type WorktreeDecisions = std::collections::HashMap<String, (switchyard_schema::Decision, u64)>;

/// Replace the stored decisions of the worktree at `worktree_path` with `decisions`.
pub(crate) fn save_decisions(db: &Connection, worktree_path: &str, decisions: &WorktreeDecisions) -> rusqlite::Result<()> {
    batch(db, |db| {
        db.execute("DELETE FROM review_decisions WHERE worktree_path = ?1", params![worktree_path])?;
        for (path, (decision, fingerprint)) in decisions {
            db.execute(
                "INSERT INTO review_decisions (worktree_path, path, decision, fingerprint) VALUES (?1, ?2, ?3, ?4)",
                params![worktree_path, path, json(decision), *fingerprint as i64],
            )?;
        }
        Ok(())
    })
}

/// Every stored decision, by worktree path.
pub(crate) fn decisions(db: &Connection) -> rusqlite::Result<std::collections::HashMap<String, WorktreeDecisions>> {
    let mut stmt = db.prepare("SELECT worktree_path, path, decision, fingerprint FROM review_decisions")?;
    let rows = stmt.query_map([], |r| {
        let decision: String = r.get(2)?;
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, decision, r.get::<_, i64>(3)? as u64))
    })?;
    let mut out: std::collections::HashMap<String, WorktreeDecisions> = Default::default();
    for row in rows {
        let (worktree, path, decision, fingerprint) = row?;
        let Ok(decision) = serde_json::from_str(&decision) else { continue };
        out.entry(worktree).or_default().insert(path, (decision, fingerprint));
    }
    Ok(out)
}

/// One worktree's hunk reviews, by hunk id.
pub(crate) type WorktreeHunkReviews = std::collections::HashMap<String, crate::review::HunkReview>;

/// Replace the worktree's stored hunk reviews with `reviews`.
pub(crate) fn save_hunk_reviews(db: &Connection, worktree_path: &str, reviews: &WorktreeHunkReviews) -> rusqlite::Result<()> {
    batch(db, |db| {
        db.execute("DELETE FROM review_hunks WHERE worktree_path = ?1", params![worktree_path])?;
        for (hunk_id, review) in reviews {
            db.execute(
                "INSERT INTO review_hunks (worktree_path, hunk_id, decision, comment, sent) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![worktree_path, hunk_id, review.decision.as_ref().map(json), review.comment, review.sent],
            )?;
        }
        Ok(())
    })
}

/// Every stored hunk review, by worktree path.
pub(crate) fn hunk_reviews(db: &Connection) -> rusqlite::Result<std::collections::HashMap<String, WorktreeHunkReviews>> {
    let mut stmt = db.prepare("SELECT worktree_path, hunk_id, decision, comment, sent FROM review_hunks")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, bool>(4)?,
        ))
    })?;
    let mut out: std::collections::HashMap<String, WorktreeHunkReviews> = Default::default();
    for row in rows {
        let (worktree, hunk_id, decision, comment, sent) = row?;
        let decision = match decision {
            Some(d) => match serde_json::from_str(&d) {
                Ok(d) => Some(d),
                Err(_) => continue,
            },
            None => None,
        };
        out.entry(worktree).or_default().insert(hunk_id, crate::review::HunkReview { decision, comment, sent });
    }
    Ok(out)
}
