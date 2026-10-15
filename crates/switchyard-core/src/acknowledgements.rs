//! Acknowledgements (ADR-0006): the user's local yes to something a Repository's committed
//! files request, pinned to a SHA-256 of exactly what was approved. Kept in the store,
//! never in the Repository. A subject is `policy` (the policy file's privilege-expanding
//! parts) or `mcp:<server name>` (that server's definition).

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS acknowledgements (
    repository_id TEXT NOT NULL,
    subject       TEXT NOT NULL,
    hash          TEXT NOT NULL,
    PRIMARY KEY (repository_id, subject)
);
";

pub(crate) fn init(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(SCHEMA)
}

/// Hex SHA-256 of `text`.
pub(crate) fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The hash the user acknowledged for `subject`, if any.
pub(crate) fn get(db: &Connection, repository_id: &str, subject: &str) -> Option<String> {
    db.query_row(
        "SELECT hash FROM acknowledgements WHERE repository_id = ?1 AND subject = ?2",
        params![repository_id, subject],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Whether `hash` is what the user acknowledged for `subject`.
pub(crate) fn holds(db: &Connection, repository_id: &str, subject: &str, hash: &str) -> bool {
    get(db, repository_id, subject).as_deref() == Some(hash)
}

pub(crate) fn set(db: &Connection, repository_id: &str, subject: &str, hash: &str) -> Result<(), String> {
    db.execute(
        "INSERT INTO acknowledgements (repository_id, subject, hash) VALUES (?1, ?2, ?3)
         ON CONFLICT(repository_id, subject) DO UPDATE SET hash = excluded.hash",
        params![repository_id, subject, hash],
    )
    .map(|_| ())
    .map_err(|e| format!("could not save it: {e}"))
}

pub(crate) fn remove(db: &Connection, repository_id: &str, subject: &str) -> Result<(), String> {
    db.execute("DELETE FROM acknowledgements WHERE repository_id = ?1 AND subject = ?2", params![repository_id, subject])
        .map(|_| ())
        .map_err(|e| format!("could not save it: {e}"))
}
