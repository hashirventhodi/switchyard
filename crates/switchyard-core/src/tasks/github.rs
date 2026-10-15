//! GitHub Issues as a TaskSource, through the Forge (`gh`). Sync ([`sync`]) reads the
//! repository's issues and keeps what it saw in the store until the next sync, so reading
//! the board never runs `gh` and the board survives a relaunch or a signed-out `gh`.
//!
//! GitHub knows only open and closed, so the column is a lossy, explicit mapping:
//! To do = open with no branch; In progress = open with a linked branch (a worktree branch
//! named for the issue, `<kind>/412-…` or `412-…`, or a branch a session linked to it ran
//! on); In review = that branch's pull request is open; Done = closed. The only write is
//! closing the issue, with a comment linking the pull request, when the work is Merged.
//! Labels, bodies and checklists are never written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection};
use switchyard_schema::{TaskColumn, TaskKind, TaskSourceKind};

use super::{SourceTask, TaskSource};
use crate::forge::{Forge, ForgeIssue};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS github_issues (
    repository_id TEXT NOT NULL,
    number        INTEGER NOT NULL,
    issue         TEXT NOT NULL,
    PRIMARY KEY (repository_id, number)
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

/// The issue a branch is named for: `feat/412-login`, `412-login` or `412`.
pub(crate) fn named_issue(branch: &str) -> Option<u32> {
    let last = branch.rsplit('/').next()?;
    let digits: String = last.chars().take_while(char::is_ascii_digit).collect();
    let rest = &last[digits.len()..];
    if digits.is_empty() || !(rest.is_empty() || rest.starts_with('-')) {
        return None;
    }
    digits.parse().ok()
}

pub(crate) struct GithubIssues<'a> {
    pub db: &'a Connection,
    pub forge: Arc<dyn Forge>,
    pub repository_id: String,
    /// The primary worktree, where `gh` runs.
    pub dir: PathBuf,
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

    /// In progress and In review follow Switchyard's facts and are never written; Done
    /// closes the issue with a comment linking the pull request.
    fn move_to(&mut self, key: &str, column: TaskColumn, pull_request: Option<&str>) -> Result<(), String> {
        if column != TaskColumn::Done {
            return Ok(());
        }
        let number = number_of(key).ok_or_else(|| format!("{key} is not an issue"))?;
        let mut issues = load(self.db, &self.repository_id);
        let Some(issue) = issues.iter_mut().find(|i| i.number == number) else {
            return Err(format!("there is no issue {key}"));
        };
        if !issue.open {
            return Ok(());
        }
        let comment = match pull_request {
            Some(url) => format!("Merged in {url}."),
            None => "Merged.".to_string(),
        };
        self.forge.close_issue(Path::new(&self.dir), number, &comment)?;
        issue.open = false;
        save(self.db, &self.repository_id, &issues)
    }
}
