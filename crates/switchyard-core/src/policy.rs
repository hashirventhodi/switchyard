//! The PolicyEngine (ADR-0004): what may run without asking in a Repository, layered
//! organisation → repository (`.switchyard/policy.yml`, via [`crate::repo_policy`]) →
//! the user's local Grants (the store). A denial in any layer wins over an allow in any
//! other. The organisation layer is modelled (`PolicyLayer::Org`) but not synced in M1.
//!
//! Policy is resolved fresh for every decision: the file is in git and may change under
//! us, and git is the source of truth. Resolved Grants are also translated into the CLI's
//! own flags at spawn ([`cli_grants`]) so it stops asking; nothing is ever written into a
//! CLI's settings files.

mod dangerous;
mod pattern;

use std::path::Path;

use adapter_protocol::RequestDetail;
use switchyard_schema::command_name;
use switchyard_schema::{
    AgentKind, Event, Grant, GrantEffect, GrantRule, PermissionMode, Policy, PolicyLayer, PolicyRequest,
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
    let (allow, deny) = file.grants().unwrap_or_default();
    // Committed allows are a request until acknowledged (ADR-0006); denials apply at once.
    let pending = pending_request(inner, &repo.id, &file);
    let mut grants = Vec::new();
    let repo_grant = |kind: &str, i: usize, effect, rule| Grant {
        id: format!("repo-{kind}-{}", i + 1),
        layer: PolicyLayer::Repo,
        effect,
        rule,
    };
    let allow: Vec<Grant> =
        allow.into_iter().enumerate().map(|(i, r)| repo_grant("allow", i, GrantEffect::Allow, r)).collect();
    if pending.is_none() {
        grants.extend(allow.iter().cloned());
    }
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
    let request = pending.map(|(hash, requested)| PolicyRequest {
        hash,
        allow,
        sandbox: requested.sandbox,
        default_mode: requested.default_mode,
        worktrees_root: requested.worktrees_root,
        sandbox_evidence: if requested.sandbox { (inner.runtime.sandbox_evidence)() } else { None },
    });
    let default_mode: Option<PermissionMode> = file.permissions.default_mode.map(Into::into);
    Policy {
        // A requested default above Ask first waits; Ask first meanwhile.
        default_mode: match &request {
            Some(r) if r.default_mode.is_some() => Some(PermissionMode::AskFirst),
            _ => default_mode,
        },
        ceiling: file.permissions.ceiling.map(Into::into),
        grants,
        error: None,
        request,
    }
}

/// The privilege-expanding parts of a committed policy file (ADR-0006).
struct Requested {
    allow: Vec<GrantRule>,
    sandbox: bool,
    default_mode: Option<PermissionMode>,
    worktrees_root: Option<String>,
}

const POLICY_SUBJECT: &str = "policy";

/// What the file requests and its hash; none when it requests nothing.
fn requested(file: &repo_policy::RepoPolicy) -> Option<(String, Requested)> {
    let (allow, _) = file.grants().unwrap_or_default();
    let default_mode = file
        .permissions
        .default_mode
        .map(PermissionMode::from)
        .filter(|m| rank(*m) > rank(PermissionMode::AskFirst));
    let requested = Requested {
        allow,
        sandbox: file.permissions.sandbox,
        default_mode,
        worktrees_root: file.worktrees.root.clone(),
    };
    if requested.allow.is_empty() && !requested.sandbox && requested.default_mode.is_none() && requested.worktrees_root.is_none() {
        return None;
    }
    let canonical = serde_json::json!({
        "allow": requested.allow,
        "sandbox": requested.sandbox,
        "defaultMode": requested.default_mode,
        "worktreesRoot": requested.worktrees_root,
    });
    Some((crate::acknowledgements::hash(&canonical.to_string()), requested))
}

/// The file's request when the user has not acknowledged exactly it.
fn pending_request(inner: &Inner, repository_id: &str, file: &repo_policy::RepoPolicy) -> Option<(String, Requested)> {
    requested(file).filter(|(hash, _)| !crate::acknowledgements::holds(inner.store.db(), repository_id, POLICY_SUBJECT, hash))
}

/// Whether everything the file requests is acknowledged (or it requests nothing).
pub(crate) fn acknowledged(inner: &Inner, repository_id: &str, file: &repo_policy::RepoPolicy) -> bool {
    pending_request(inner, repository_id, file).is_none()
}

/// One line naming what a pending request holds back, for a session's transcript.
pub(crate) fn pending_summary(inner: &Inner, repo: &StoredRepository) -> Option<String> {
    let file = repo_policy::load(Path::new(&repo.path)).ok()?;
    let (_, r) = pending_request(inner, &repo.id, &file)?;
    let mut parts = Vec::new();
    match r.allow.len() {
        0 => {}
        1 => parts.push("1 allow rule".to_string()),
        n => parts.push(format!("{n} allow rules")),
    }
    if r.sandbox {
        parts.push("a sandbox (no bypass in Full auto)".into());
    }
    if let Some(mode) = r.default_mode {
        parts.push(format!("default mode {}", mode_label(mode)));
    }
    if r.worktrees_root.is_some() {
        parts.push("its own worktree root".into());
    }
    Some(format!(
        "The repository's committed policy requests {} that do not apply until you acknowledge it in Context.",
        parts.join(", ")
    ))
}

/// The `acknowledgePolicy` command.
pub(crate) fn acknowledge(inner: &Inner, repository_id: &str, hash: &str) {
    const COMMAND: &str = command_name::ACKNOWLEDGE_POLICY;
    let Some(repo) = repository(inner, repository_id) else {
        return inner.refuse(COMMAND, crate::no_repository(repository_id));
    };
    let file = match repo_policy::load(Path::new(&repo.path)) {
        Ok(file) => file,
        Err(error) => return inner.refuse(COMMAND, error),
    };
    let Some((current, _)) = requested(&file) else {
        return inner.refuse(COMMAND, "The repository's policy requests nothing to acknowledge".into());
    };
    if current != hash {
        return inner.refuse(
            COMMAND,
            "The repository's policy changed since you read it; read it again before acknowledging".into(),
        );
    }
    if let Err(reason) = crate::acknowledgements::set(inner.store.db(), &repo.id, POLICY_SUBJECT, hash) {
        return inner.refuse(COMMAND, format!("Could not acknowledge the policy: {reason}"));
    }
    emit(inner, &repo);
}

/// What on this machine supports a sandbox claim: Switchyard itself running in a
/// container or VM. `None` when nothing is detected (the claim cannot be verified).
pub fn detect_sandbox() -> Option<String> {
    if Path::new("/.dockerenv").exists() {
        return Some("Switchyard is running in a Docker container (/.dockerenv)".into());
    }
    if Path::new("/run/.containerenv").exists() {
        return Some("Switchyard is running in a Podman container (/run/.containerenv)".into());
    }
    if let Ok(kind) = std::env::var("container") {
        return Some(format!("Switchyard is running in a container ({kind})"));
    }
    if let Ok(cgroup) = std::fs::read_to_string("/proc/1/cgroup") {
        if ["docker", "kubepods", "lxc", "containerd"].iter().any(|k| cgroup.contains(k)) {
            return Some("Switchyard is running in a container (/proc/1/cgroup)".into());
        }
    }
    None
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
/// The rule Always allow saves for `request` in a session in `mode`: `pattern` (the user's,
/// or the suggestion when none). Refused when it would not answer the request next time,
/// so a card never says "Always allowed" for a grant that will not apply: it must cover the
/// command, and below Full auto name each dangerous class the command is in.
pub(crate) fn grant_for(request: &RequestDetail, pattern: Option<&str>, mode: PermissionMode) -> Result<GrantRule, String> {
    match request {
        RequestDetail::Command { command, .. } => {
            let pattern = match pattern.map(pattern::squash).filter(|p| !p.is_empty()) {
                Some(p) => p,
                None => pattern::suggest(command),
            };
            if pattern::matches(&pattern, command) {
                if guards_dangerous(mode) && !dangerous::explicitly_covers(&pattern, command) {
                    let literal = &pattern[..pattern.find('*').unwrap_or(pattern.len())];
                    let named = dangerous::classes(literal);
                    let class = dangerous::classes(command).into_iter().find(|c| !named.contains(c)).expect("uncovered");
                    return Err(format!(
                        "{pattern} would not answer {}: below Full auto a {} command is granted only by a pattern that names it, such as {}",
                        pattern::squash(command),
                        class.label(),
                        pattern::suggest(command),
                    ));
                }
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
pub(crate) fn cli_grants(agent: AgentKind, policy: &Policy) -> (Vec<String>, Vec<String>) {
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

/// Whether the repository's policy declares a sandbox the user has acknowledged (a broken
/// file declares nothing; an unacknowledged declaration is only a request, ADR-0006).
pub(crate) fn sandboxed(inner: &Inner, repo: &StoredRepository) -> bool {
    repo_policy::load(Path::new(&repo.path))
        .is_ok_and(|file| file.permissions.sandbox && acknowledged(inner, &repo.id, &file))
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
        None => inner.refuse(command_name::READ_POLICY, crate::no_repository(&repository_id)),
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
    const COMMAND: &str = command_name::REVOKE_GRANT;
    let Some(repo) = repository(inner, repository_id) else {
        return inner.refuse(COMMAND, crate::no_repository(&repository_id));
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
