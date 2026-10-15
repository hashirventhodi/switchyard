//! Tasks: the reason a Session exists. Each comes from a TaskSource (local tasks kept per
//! Repository; GitHub Issues next) and sits in one column of the Tasks board.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A Task as the Tasks board and Context show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Task {
    /// Unique across Repositories and sources; opaque to the renderer.
    pub id: String,
    pub repository_id: String,
    pub source: TaskSourceKind,
    /// What the source calls it: `L-3` for a local task, `#412` for an issue.
    pub key: String,
    pub title: String,
    pub description: String,
    /// The branch kind a session for it starts on: `<kind>/<key>-<slug>`.
    pub kind: TaskKind,
    pub column: TaskColumn,
    /// Sub-steps, ticked locally. Ticks never leave Switchyard's store.
    pub checklist: Vec<ChecklistItem>,
    /// Where the task lives on its tracker, when it has an address.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TaskSourceKind {
    /// Kept by Switchyard in a JSON file per Repository.
    Local,
    /// GitHub Issues, through the Forge.
    Github,
}

/// The branch kind for a task's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TaskKind {
    Feat,
    Fix,
    Chore,
}

/// The four columns of the Tasks board. A task only moves forward: To do → In progress
/// (a session starts on it) → In review (its pull request opens) → Done (Merged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TaskColumn {
    Todo,
    InProgress,
    InReview,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChecklistItem {
    pub text: String,
    pub done: bool,
}

/// Where [`crate::Command::StartTaskSession`] runs the session: the same three choices New
/// session offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum TaskSessionTarget {
    /// A new worktree from base in the task's Repository, on `branch`.
    NewWorktree { branch: String },
    /// An existing free worktree.
    Worktree {
        #[serde(rename = "worktreePath")]
        #[ts(rename = "worktreePath")]
        worktree_path: String,
    },
    /// Stop and replace the Paused, Finished or Error session `sessionId` (archived).
    Replace {
        #[serde(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: String,
    },
}
