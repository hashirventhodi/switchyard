//! The Repository's committed policy, `.switchyard/policy.yml` in its primary worktree.
//! It is reviewed in pull requests and versioned with the code; the PolicyEngine
//! (`crate::policy`) layers it with the user's local Grants.
//!
//! ```yaml
//! worktrees:
//!   root: ../acme-worktrees   # relative to the repository; `~/` is the home folder
//! permissions:
//!   default_mode: auto_edit   # explore | ask_first | auto_edit | full_auto
//!   ceiling: auto_edit        # no Session may run in a more autonomous mode
//!   allow:
//!     - command: pnpm *       # a command prefix plus a glob on the rest
//!     - network: registry.npmjs.org
//!     - mcp: { server: github, tool: get_issue }   # tool `*`: every tool of the server
//!   deny:                     # wins over any allow, in any layer
//!     - command: git push *
//!   sandbox: true             # sessions run sandboxed: Full auto bypasses the CLI's checks
//! secrets:                    # environment variable names a Session may be granted
//!   - NPM_TOKEN               # (values live in the keychain or environment, never here)
//! ```
//!
//! Unknown top-level keys are ignored; a rule must name exactly one of `command`,
//! `network` or `mcp`, so a misspelt denial is an error rather than silently dropped.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use switchyard_schema::{GrantRule, PermissionMode};

pub const POLICY_FILE: &str = ".switchyard/policy.yml";

#[derive(Debug, Default, Deserialize)]
pub struct RepoPolicy {
    #[serde(default)]
    pub worktrees: WorktreesPolicy,
    #[serde(default)]
    pub permissions: PermissionsPolicy,
    /// Secret names (environment variables) the repository's Sessions may be granted.
    /// Values never go in this file.
    #[serde(default)]
    pub secrets: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PermissionsPolicy {
    pub default_mode: Option<ModeName>,
    pub ceiling: Option<ModeName>,
    #[serde(default)]
    pub allow: Vec<RuleSpec>,
    #[serde(default)]
    pub deny: Vec<RuleSpec>,
    /// Sessions run in a sandbox: only then does Full auto use the CLI's
    /// bypass-everything mode.
    #[serde(default)]
    pub sandbox: bool,
}

/// A Permission mode as the file spells it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeName {
    Explore,
    AskFirst,
    AutoEdit,
    FullAuto,
}

impl From<ModeName> for PermissionMode {
    fn from(mode: ModeName) -> Self {
        match mode {
            ModeName::Explore => PermissionMode::Explore,
            ModeName::AskFirst => PermissionMode::AskFirst,
            ModeName::AutoEdit => PermissionMode::AutoEdit,
            ModeName::FullAuto => PermissionMode::FullAuto,
        }
    }
}

/// One grant or denial: exactly one of its fields.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSpec {
    pub command: Option<String>,
    pub network: Option<String>,
    pub mcp: Option<McpSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpSpec {
    pub server: String,
    pub tool: String,
}

impl RuleSpec {
    fn rule(&self) -> Result<GrantRule, String> {
        let non_empty = |s: &str| !s.trim().is_empty();
        match (&self.command, &self.network, &self.mcp) {
            (Some(pattern), None, None) if non_empty(pattern) => Ok(GrantRule::Command { pattern: pattern.trim().into() }),
            (None, Some(domain), None) if non_empty(domain) => {
                Ok(GrantRule::Network { domain: domain.trim().to_ascii_lowercase() })
            }
            (None, None, Some(m)) if non_empty(&m.server) && non_empty(&m.tool) => {
                Ok(GrantRule::McpTool { server: m.server.trim().into(), tool: m.tool.trim().into() })
            }
            _ => Err("each rule names exactly one of command, network or mcp".into()),
        }
    }
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
    let policy: RepoPolicy = serde_yaml::from_str(&text).map_err(|e| format!("{POLICY_FILE} is not valid: {e}"))?;
    policy.rules().map_err(|e| format!("{POLICY_FILE} is not valid: {e}"))?;
    Ok(policy)
}

impl RepoPolicy {
    /// The file's allow rules and deny rules, in file order.
    pub fn rules(&self) -> Result<(Vec<GrantRule>, Vec<GrantRule>), String> {
        let read = |list: &[RuleSpec], key: &str| -> Result<Vec<GrantRule>, String> {
            list.iter().enumerate().map(|(i, r)| r.rule().map_err(|e| format!("permissions.{key}[{i}]: {e}"))).collect()
        };
        Ok((read(&self.permissions.allow, "allow")?, read(&self.permissions.deny, "deny")?))
    }

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
