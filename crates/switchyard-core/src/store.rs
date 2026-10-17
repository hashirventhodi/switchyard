//! The core's SQLite store. Holds only what git cannot tell us: which repositories the
//! user added, and the user's local Grants for each (sessions are in `session_store`).
//! Worktrees and branches are never stored; they are re-read from git.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use switchyard_schema::GrantRule;

pub struct Store {
    db: Connection,
}

#[derive(Clone)]
pub struct StoredRepository {
    pub id: String,
    pub path: String,
}

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE repositories (
        id   INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE
    );",
    // Local Grants (ADR-0004): `rule` is the schema GrantRule as JSON.
    "CREATE TABLE grants (
        id            INTEGER PRIMARY KEY,
        repository_id TEXT NOT NULL,
        rule          TEXT NOT NULL
    );",
];

pub struct StoredGrant {
    pub id: String,
    pub rule: GrantRule,
}

fn grant_id(rowid: i64) -> String {
    format!("grant-{rowid}")
}

fn repo_id(rowid: i64) -> String {
    format!("repo-{rowid}")
}

impl Store {
    pub fn open(data_dir: &Path) -> rusqlite::Result<Store> {
        let db = Connection::open(data_dir.join("switchyard.db"))?;
        // Sessions write on every agent event: WAL keeps those writes cheap and crash-safe.
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        let version: usize = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            db.execute_batch(sql)?;
            db.pragma_update(None, "user_version", i + 1)?;
        }
        crate::session_store::init(&db)?;
        crate::tasks::init(&db)?;
        crate::secrets::init(&db)?;
        crate::acknowledgements::init(&db)?;
        crate::models::init(&db)?;
        Ok(Store { db })
    }

    pub(crate) fn db(&self) -> &Connection {
        &self.db
    }

    pub fn repository_by_path(&self, path: &str) -> rusqlite::Result<Option<StoredRepository>> {
        self.db
            .query_row("SELECT id FROM repositories WHERE path = ?1", params![path], |r| {
                Ok(StoredRepository { id: repo_id(r.get(0)?), path: path.to_string() })
            })
            .optional()
    }

    pub fn insert_repository(&self, path: &str) -> rusqlite::Result<StoredRepository> {
        self.db.execute("INSERT INTO repositories (path) VALUES (?1)", params![path])?;
        Ok(StoredRepository { id: repo_id(self.db.last_insert_rowid()), path: path.to_string() })
    }

    pub fn repositories(&self) -> rusqlite::Result<Vec<StoredRepository>> {
        let mut stmt = self.db.prepare("SELECT id, path FROM repositories ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(StoredRepository { id: repo_id(r.get(0)?), path: r.get(1)? })
        })?;
        rows.collect()
    }

    /// The user's local Grants for a repository, oldest first. Rows that no longer read
    /// as a rule are skipped.
    pub fn grants(&self, repository_id: &str) -> rusqlite::Result<Vec<StoredGrant>> {
        let mut stmt = self.db.prepare("SELECT id, rule FROM grants WHERE repository_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map(params![repository_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, rule) = row?;
            if let Ok(rule) = serde_json::from_str(&rule) {
                out.push(StoredGrant { id: grant_id(id), rule });
            }
        }
        Ok(out)
    }

    pub fn insert_grant(&self, repository_id: &str, rule: &GrantRule) -> rusqlite::Result<StoredGrant> {
        let json = serde_json::to_string(rule).expect("grant rules serialize");
        self.db.execute("INSERT INTO grants (repository_id, rule) VALUES (?1, ?2)", params![repository_id, json])?;
        Ok(StoredGrant { id: grant_id(self.db.last_insert_rowid()), rule: rule.clone() })
    }

    /// Delete a local Grant of the repository; false when there is none with that id.
    pub fn delete_grant(&self, repository_id: &str, id: &str) -> rusqlite::Result<bool> {
        let Some(rowid) = id.strip_prefix("grant-").and_then(|n| n.parse::<i64>().ok()) else { return Ok(false) };
        let n = self.db.execute("DELETE FROM grants WHERE id = ?1 AND repository_id = ?2", params![rowid, repository_id])?;
        Ok(n > 0)
    }
}
