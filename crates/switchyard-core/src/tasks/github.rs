//! GitHub Issues as a TaskSource, through the Forge (`gh`). Sync ([`sync`]) reads the
//! repository's issues and keeps what it saw in the store until the next sync, so reading
//! the board never runs `gh` and the board survives a relaunch or a signed-out `gh`.
//!
//! GitHub knows only open and closed, so the column is a lossy, explicit mapping:
//! To do = open with no linked session; In progress = open with a linked session (a link
//! the user made, never a branch-name guess); In review = that session's worktree has an
//! open pull request; Done = closed on GitHub, however it was closed. Switchyard writes
//! nothing to the tracker: a pull request it opens for a linked issue says "Closes #N",
//! and GitHub closes the issue when it merges.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use switchyard_schema::{TaskColumn, TaskKind, TaskSourceKind};

use super::{SourceTask, TaskSource};
use crate::forge::ForgeIssue;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS github_issues (
    repository_id TEXT NOT NULL,
    number        INTEGER NOT NULL,
    issue         TEXT NOT NULL,
    PRIMARY KEY (repository_id, number)
);
CREATE TABLE IF NOT EXISTS github_checked (
    repository_id TEXT PRIMARY KEY,
    checked_at_ms INTEGER NOT NULL
);
";

pub(crate) fn init(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(SCHEMA)
}

/// Replace what the last sync saw with `issues` (newest first).
pub(crate) fn save(db: &Connection, repository_id: &str, issues: &[ForgeIssue]) -> Result<(), String> {
    let write = || -> rusqlite::Result<()> {
        let tx = db.unchecked_transaction()?;
        tx.execute("DELETE FROM github_issues WHERE repository_id = ?1", params![repository_id])?;
        for issue in issues {
            let json = serde_json::to_string(issue).expect("issues serialize");
            tx.execute(
                "INSERT OR REPLACE INTO github_issues (repository_id, number, issue) VALUES (?1, ?2, ?3)",
                params![repository_id, issue.number, json],
            )?;
        }
        tx.commit()
    };
    write().map_err(|e| format!("could not keep the issues: {e}"))
}

/// Whether the repository's issues were ever synced (a sync that found none counts).
pub(crate) fn synced(db: &Connection, repository_id: &str) -> bool {
    checked_at(db, repository_id).is_some() || !load(db, repository_id).is_empty()
}

/// The issues were read from the Forge at `at_ms`.
pub(crate) fn checked(db: &Connection, repository_id: &str, at_ms: u64) -> Result<(), String> {
    db.execute(
        "INSERT OR REPLACE INTO github_checked (repository_id, checked_at_ms) VALUES (?1, ?2)",
        params![repository_id, at_ms as i64],
    )
    .map(|_| ())
    .map_err(|e| format!("could not keep the issues: {e}"))
}

/// When the issues were last read from the Forge, if ever.
pub(crate) fn checked_at(db: &Connection, repository_id: &str) -> Option<u64> {
    db.query_row("SELECT checked_at_ms FROM github_checked WHERE repository_id = ?1", params![repository_id], |r| {
        r.get::<_, i64>(0)
    })
    .ok()
    .map(|ms| ms as u64)
}

/// Do `issues` differ from what the last sync saw?
pub(crate) fn differ(db: &Connection, repository_id: &str, issues: &[ForgeIssue]) -> bool {
    load(db, repository_id) != issues
}

fn load(db: &Connection, repository_id: &str) -> Vec<ForgeIssue> {
    let read = || -> rusqlite::Result<Vec<String>> {
        let mut stmt = db.prepare("SELECT issue FROM github_issues WHERE repository_id = ?1 ORDER BY rowid")?;
        let rows = stmt.query_map(params![repository_id], |r| r.get(0))?;
        rows.collect()
    };
    read().unwrap_or_default().iter().filter_map(|j| serde_json::from_str(j).ok()).collect()
}

/// `#412` → 412.
pub(crate) fn number_of(key: &str) -> Option<u32> {
    key.strip_prefix('#')?.parse().ok()
}

pub(crate) struct GithubIssues<'a> {
    pub db: &'a Connection,
    pub repository_id: String,
    /// How far each open issue's work has got by Switchyard's own facts (a linked branch:
    /// In progress; its pull request open: In review), by number.
    pub progress: HashMap<u32, TaskColumn>,
}

fn kind_of(labels: &[String]) -> TaskKind {
    let has = |names: &[&str]| labels.iter().any(|l| names.contains(&l.to_ascii_lowercase().as_str()));
    if has(&["bug", "fix", "regression"]) {
        TaskKind::Fix
    } else if has(&["chore", "maintenance", "dependencies", "refactor"]) {
        TaskKind::Chore
    } else {
        TaskKind::Feat
    }
}

/// The body's task list (`- [ ] step`, ticked or not) as checklist text; ticks stay local.
fn checklist(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l.strip_prefix("- ").or_else(|| l.strip_prefix("* "))?;
            let text = ["[ ] ", "[x] ", "[X] "].iter().find_map(|m| rest.strip_prefix(m))?;
            Some(text.trim().to_string()).filter(|t| !t.is_empty())
        })
        .collect()
}

impl GithubIssues<'_> {
    fn source_task(&self, issue: &ForgeIssue) -> SourceTask {
        let column = if issue.open {
            self.progress.get(&issue.number).copied().unwrap_or(TaskColumn::Todo)
        } else {
            TaskColumn::Done
        };
        SourceTask {
            key: format!("#{}", issue.number),
            title: issue.title.clone(),
            description: issue.body.trim().to_string(),
            kind: kind_of(&issue.labels),
            column,
            checklist: checklist(&issue.body),
            url: Some(issue.url.clone()),
        }
    }
}

impl TaskSource for GithubIssues<'_> {
    fn kind(&self) -> TaskSourceKind {
        TaskSourceKind::Github
    }

    fn tasks(&self) -> Result<Vec<SourceTask>, String> {
        Ok(load(self.db, &self.repository_id).iter().map(|i| self.source_task(i)).collect())
    }

    /// Nothing is written: every column follows Switchyard's facts or GitHub's own state
    /// (a merged pull request that said "Closes #N" closes the issue on GitHub).
    fn move_to(&mut self, _key: &str, _column: TaskColumn, _pull_request: Option<&str>) -> Result<(), String> {
        Ok(())
    }
}
