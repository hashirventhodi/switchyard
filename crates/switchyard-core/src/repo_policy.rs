//! The Repository's committed policy, `.switchyard/policy.yml` in its primary worktree.
//! Only the worktree root is read so far; the PolicyEngine (grants, denials, modes and the
//! ceiling) extends [`RepoPolicy`] rather than parsing the file a second time.
//!
//! ```yaml
//! worktrees:
//!   root: ../acme-worktrees   # relative to the repository; `~/` is the home folder
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const POLICY_FILE: &str = ".switchyard/policy.yml";

#[derive(Debug, Default, Deserialize)]
pub struct RepoPolicy {
    #[serde(default)]
    pub worktrees: WorktreesPolicy,
}

#[derive(Debug, Default, Deserialize)]
pub struct WorktreesPolicy {
    /// Where this repository's worktrees go, instead of the Switchyard worktree root.
    pub root: Option<String>,
}

/// The policy of the repository whose primary worktree is `primary`. A missing file is
/// an empty policy; a file that cannot be read or parsed is an error naming it.
pub fn load(primary: &Path) -> Result<RepoPolicy, String> {
    let file = primary.join(POLICY_FILE);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RepoPolicy::default()),
        Err(e) => return Err(format!("cannot read {POLICY_FILE}: {e}")),
    };
    if text.trim().is_empty() {
        return Ok(RepoPolicy::default());
    }
    serde_yaml::from_str(&text).map_err(|e| format!("{POLICY_FILE} is not valid: {e}"))
}

impl RepoPolicy {
    /// The worktree root this policy names, resolved against the repository's primary
    /// worktree, if it names one.
    pub fn worktree_root(&self, primary: &Path) -> Option<PathBuf> {
        let root = self.worktrees.root.as_deref()?.trim();
        if root.is_empty() {
            return None;
        }
        let path = match root.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
            None => primary.join(root),
        };
        Some(normalize(&path))
    }
}

/// Drop `.` and resolve `..` lexically, so the path reads as git will list it.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
