//! The core's SQLite store. Holds only what git cannot tell us: which repositories the
//! user added. Worktrees and branches are never stored; they are re-read from git.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

pub struct Store {
    db: Connection,
}

pub struct StoredRepository {
    pub id: String,
    pub path: String,
}

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE repositories (
        id   INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE
    );",
];

fn repo_id(rowid: i64) -> String {
    format!("repo-{rowid}")
}

impl Store {
    pub fn open(data_dir: &Path) -> rusqlite::Result<Store> {
        let db = Connection::open(data_dir.join("switchyard.db"))?;
        let version: usize = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            db.execute_batch(sql)?;
            db.pragma_update(None, "user_version", i + 1)?;
        }
        Ok(Store { db })
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
}
