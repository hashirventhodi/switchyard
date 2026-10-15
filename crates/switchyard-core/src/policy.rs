//! The PolicyEngine (ADR-0004): what may run without asking in a Repository, layered
//! organisation → repository (`.switchyard/policy.yml`, via [`crate::repo_policy`]) →
//! the user's local Grants (the store). A denial in any layer wins over an allow in any
//! other. The organisation layer is modelled (`PolicyLayer::Org`) but not synced in M1.
//!
//! Policy is resolved fresh for every decision: the file is in git and may change under
//! us, and git is the source of truth. Resolved Grants are also translated into the CLI's
//! own flags at spawn ([`cli_rules`]) so it stops asking; nothing is ever written into a
//! CLI's settings files.

mod dangerous;
mod pattern;

use std::path::Path;

use adapter_protocol::RequestDetail;
use switchyard_schema::{
    AgentKind, Event, Grant, GrantEffect, GrantRule, PermissionMode, Policy, PolicyLayer,
};

use crate::store::StoredRepository;
use crate::{repo_policy, Inner};

/// What the policy says about a request.
pub(crate) enum Decision {
    /// Ask the user.
    Ask,
    Allow(Grant),
    Deny(Grant),
}

/// The repository's policy, resolved across layers. When the policy file cannot be read,
/// no grant applies (its denials are unknown) and `error` says why.
pub(crate) fn resolve(inner: &Inner, repo: &StoredRepository) -> Policy {
    let file = match repo_policy::load(Path::new(&repo.path)) {
        Ok(file) => file,
        Err(error) => return Policy { error: Some(error), ..Policy::default() },
    };
    let (allow, deny) = file.rules().unwrap_or_default();
    let mut grants = Vec::new();
    let repo_grant = |kind: &str, i: usize, effect, rule| Grant {
        id: format!("repo-{kind}-{}", i + 1),
        layer: PolicyLayer::Repo,
        effect,
        rule,
    };
    grants.extend(allow.into_iter().enumerate().map(|(i, r)| repo_grant("allow", i, GrantEffect::Allow, r)));
    grants.extend(deny.into_iter().enumerate().map(|(i, r)| repo_grant("deny", i, GrantEffect::Deny, r)));
    match inner.store.grants(&repo.id) {
        Ok(local) => grants.extend(local.into_iter().map(|g| Grant {
            id: g.id,
            layer: PolicyLayer::Local,
            effect: GrantEffect::Allow,
            rule: g.rule,
        })),
        Err(e) => return Policy { error: Some(format!("cannot read your grants: {e}")), ..Policy::default() },
    }
    Policy {
        default_mode: file.permissions.default_mode.map(Into::into),
        ceiling: file.permissions.ceiling.map(Into::into),
        grants,
        error: None,
    }
}

/// Deny wins across layers; then any allow; otherwise ask. File writes and questions are
/// never answered by policy. Below Full auto, a dangerous command (network, delete,
/// migration, push) is allowed only by a grant that names it explicitly.
pub(crate) fn decide(policy: &Policy, request: &RequestDetail, mode: PermissionMode) -> Decision {
    let matching = |effect: GrantEffect| {
        policy.grants.iter().find(|g| g.effect == effect && rule_matches(&g.rule, request)).cloned()
    };
    if let Some(deny) = matching(GrantEffect::Deny) {
        return Decision::Deny(deny);
    }
    let explicit = |grant: &Grant| match (&grant.rule, request) {
        (GrantRule::Command { pattern }, RequestDetail::Command { command, .. }) if guards_dangerous(mode) => {
            dangerous::explicitly_covers(pattern, command)
        }
        _ => true,
    };
    let allow = policy
        .grants
        .iter()
        .find(|g| g.effect == GrantEffect::Allow && rule_matches(&g.rule, request) && explicit(g))
        .cloned();
    match allow {
        Some(allow) => Decision::Allow(allow),
        None => Decision::Ask,
    }
}

fn rule_matches(rule: &GrantRule, request: &RequestDetail) -> bool {
    match (rule, request) {
        (GrantRule::Command { pattern }, RequestDetail::Command { command, .. }) => pattern::matches(pattern, command),
        (GrantRule::Network { domain }, RequestDetail::Network { domain: asked, .. }) => {
            let (domain, asked) = (domain.to_ascii_lowercase(), asked.to_ascii_lowercase());
            match domain.strip_prefix("*.") {
                Some(parent) => asked.ends_with(&format!(".{parent}")),
                None => domain == asked,
            }
        }
        (GrantRule::McpTool { server, tool }, RequestDetail::McpTool { server: s, tool: t, .. }) => {
            server == s && (tool == "*" || tool == t)
        }
        _ => false,
    }
}

/// The Grant "Always allow" makes for `request`, with the user's `pattern` for a command
/// (or the suggested one). Err is the reason shown to the user.
pub(crate) fn grant_for(request: &RequestDetail, pattern: Option<&str>) -> Result<GrantRule, String> {
    match request {
        RequestDetail::Command { command, .. } => {
            let pattern = match pattern.map(pattern::squash).filter(|p| !p.is_empty()) {
                Some(p) => p,
                None => pattern::suggest(command),
            };
            if pattern::matches(&pattern, command) {
                return Ok(GrantRule::Command { pattern });
            }
            let why = if pattern.contains('*') && covers_but_for_interpreter(&pattern, command) {
                " (interpreter commands need an explicit grant)"
            } else {
                ""
            };
            Err(format!("{pattern} does not cover {}{why}", pattern::squash(command)))
        }
        RequestDetail::Network { domain, .. } => Ok(GrantRule::Network { domain: domain.to_ascii_lowercase() }),
        RequestDetail::McpTool { server, tool, .. } => Ok(GrantRule::McpTool { server: server.clone(), tool: tool.clone() }),
        RequestDetail::FileWrite { .. } => Err("File writes are never granted: Run once or Skip".into()),
        RequestDetail::Question { .. } => Err("A question needs an answer".into()),
    }
}

/// A plain glob would match, so only the interpreter rule refused it.
fn covers_but_for_interpreter(pattern: &str, command: &str) -> bool {
    let literal = &pattern[..pattern.find('*').unwrap_or(pattern.len())];
    pattern::squash(command).starts_with(literal) && pattern::names_interpreter(command)
}

/// How a rule reads on a system line: the pattern, the domain, `server · tool`.
pub(crate) fn describe(rule: &GrantRule) -> String {
    match rule {
        GrantRule::Command { pattern } => pattern.clone(),
        GrantRule::Network { domain } => domain.clone(),
        GrantRule::McpTool { server, tool } => format!("{server} · {tool}"),
    }
}

/// Whose grant it is, for a system line: "your grant", "repository policy".
pub(crate) fn source(grant: &Grant) -> &'static str {
    match grant.layer {
        PolicyLayer::Local => "your grant",
        PolicyLayer::Repo => "repository policy",
        PolicyLayer::Org => "organisation policy",
    }
}

/// The CLI's allowed-tools and disallowed-tools rules for `policy`. An allow the CLI would
/// apply more broadly than Switchyard does (a wildcard over an interpreter, a wildcard in
/// the middle) is left out: Switchyard answers those itself when the CLI asks. Denials
/// are always passed; a broader denial only asks less.
///
/// Codex has no per-invocation rule for commands or domains, so only MCP tool rules reach
/// it, as `mcp:<server>/<tool>` (`*` for every tool); the Codex launcher turns them into
/// the thread's config ([`crate::codex`]). Switchyard answers everything else it asks.
pub(crate) fn cli_rules(agent: AgentKind, policy: &Policy) -> (Vec<String>, Vec<String>) {
    let translate = match agent {
        AgentKind::ClaudeCode => claude_rule,
        AgentKind::Codex => codex_rule,
    };
    let (mut allowed, mut disallowed) = (Vec::new(), Vec::new());
    for grant in &policy.grants {
        let deny = grant.effect == GrantEffect::Deny;
        if let Some(rule) = translate(&grant.rule, deny) {
            if deny { &mut disallowed } else { &mut allowed }.push(rule);
        }
    }
    (allowed, disallowed)
}

fn codex_rule(rule: &GrantRule, _deny: bool) -> Option<String> {
    match rule {
        GrantRule::McpTool { server, tool } => Some(format!("mcp:{server}/{tool}")),
        GrantRule::Command { .. } | GrantRule::Network { .. } => None,
    }
}

fn claude_rule(rule: &GrantRule, deny: bool) -> Option<String> {
    match rule {
        GrantRule::Command { pattern } => {
            let pattern = pattern::squash(pattern);
            let Some(prefix) = pattern.strip_suffix(" *").filter(|p| !p.contains('*')) else {
                // Exact, or a wildcard that is not trailing.
                return (deny || !pattern.contains('*')).then(|| format!("Bash({pattern})"));
            };
            (deny || !pattern::names_interpreter(prefix)).then(|| format!("Bash({prefix}:*)"))
        }
        GrantRule::Network { domain } => {
            (deny || !domain.contains('*')).then(|| format!("WebFetch(domain:{domain})"))
        }
        GrantRule::McpTool { server, tool } if tool == "*" => Some(format!("mcp__{server}")),
        GrantRule::McpTool { server, tool } => Some(format!("mcp__{server}__{tool}")),
    }
}

/// Whether dangerous commands always prompt in `mode` (every mode below Full auto).
/// Requests the session's mode itself answers, for an agent whose CLI asks Switchyard more
/// than the mode needs. Codex in Auto edit runs with approval policy `untrusted` so that a
/// dangerous command can never run inside the worktree without the user (`on-request`
/// would let it); Switchyard then allows what Auto edit means: file edits and commands
/// outside the dangerous classes. Claude Code applies Auto edit itself (acceptEdits).
pub(crate) fn mode_allows(agent: AgentKind, mode: PermissionMode, request: &RequestDetail) -> bool {
    match (agent, mode, request) {
        (AgentKind::Codex, PermissionMode::AutoEdit, RequestDetail::FileWrite { .. }) => true,
        (AgentKind::Codex, PermissionMode::AutoEdit, RequestDetail::Command { command, .. }) => {
            dangerous::classes(command).is_empty()
        }
        _ => false,
    }
}

pub(crate) fn guards_dangerous(mode: PermissionMode) -> bool {
    rank(mode) < rank(PermissionMode::FullAuto)
}

/// Modes from least to most autonomous.
fn rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Explore => 0,
        PermissionMode::AskFirst => 1,
        PermissionMode::AutoEdit => 2,
        PermissionMode::FullAuto => 3,
    }
}

/// Refuses a mode above the repository's ceiling, naming the ceiling.
pub(crate) fn admit_mode(repo: &StoredRepository, mode: PermissionMode) -> Result<(), String> {
    let ceiling = match repo_policy::load(Path::new(&repo.path)) {
        Ok(file) => file.permissions.ceiling.map(PermissionMode::from),
        Err(error) => return Err(format!("Could not start the session: {error}")),
    };
    match ceiling {
        Some(ceiling) if rank(mode) > rank(ceiling) => {
            Err(format!("This repository's policy allows at most {}", mode_label(ceiling)))
        }
        _ => Ok(()),
    }
}

/// Whether the repository's policy declares a sandbox (a broken file declares nothing).
pub(crate) fn sandboxed(repo: &StoredRepository) -> bool {
    repo_policy::load(Path::new(&repo.path)).is_ok_and(|file| file.permissions.sandbox)
}

pub(crate) fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Explore => "Explore",
        PermissionMode::AskFirst => "Ask first",
        PermissionMode::AutoEdit => "Auto edit",
        PermissionMode::FullAuto => "Full auto",
    }
}

pub(crate) fn emit(inner: &Inner, repo: &StoredRepository) {
    let policy = resolve(inner, repo);
    inner.emit(Event::PolicyChanged { repository_id: repo.id.clone(), policy });
}

pub(crate) fn repository(inner: &Inner, id: &str) -> Option<StoredRepository> {
    inner.store.repositories().ok()?.into_iter().find(|r| r.id == id)
}

/// The `readPolicy` command.
pub(crate) fn read(inner: &Inner, repository_id: &str) {
    match repository(inner, repository_id) {
        Some(repo) => emit(inner, &repo),
        None => inner.refuse("readPolicy", format!("There is no repository {repository_id}")),
    }
}

/// Keep a local Grant for the repository (once: an identical one is not added twice).
pub(crate) fn add_local(inner: &Inner, repository_id: &str, rule: &GrantRule) -> Result<(), String> {
    let existing = inner.store.grants(repository_id).map_err(|e| format!("cannot read your grants: {e}"))?;
    if existing.iter().any(|g| &g.rule == rule) {
        return Ok(());
    }
    inner.store.insert_grant(repository_id, rule).map(drop).map_err(|e| format!("cannot save the grant: {e}"))
}

/// The `revokeGrant` command: local Grants only.
pub(crate) fn revoke(inner: &Inner, repository_id: &str, grant_id: &str) {
    const COMMAND: &str = "revokeGrant";
    let Some(repo) = repository(inner, repository_id) else {
        return inner.refuse(COMMAND, format!("There is no repository {repository_id}"));
    };
    if grant_id.starts_with("repo-") {
        return inner.refuse(COMMAND, "That grant is in the repository's policy file; change it there".into());
    }
    match inner.store.delete_grant(&repo.id, grant_id) {
        Ok(true) => emit(inner, &repo),
        Ok(false) => inner.refuse(COMMAND, format!("There is no grant {grant_id} in this repository")),
        Err(e) => inner.refuse(COMMAND, format!("cannot revoke the grant: {e}")),
    }
}
