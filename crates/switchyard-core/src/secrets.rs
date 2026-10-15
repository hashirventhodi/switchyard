//! Secrets grants. A Repository's secrets are environment variable NAMES: declared in its
//! `.switchyard/policy.yml` (`secrets: [NPM_TOKEN]`), referenced as `${NAME}` by a server
//! in its `.mcp.json`, or added locally by the user. The store keeps names and grant flags
//! only; values come from the OS keychain or, failing that, Switchyard's own environment,
//! through [`SecretValues`], and are read only at spawn.
//!
//! At spawn the core hands the launcher a [`SpawnEnv`]: the granted secrets with their
//! values, and every other secret name Switchyard knows of (any repository's) to strip
//! from the inherited environment. The agent's CLI, and the MCP server processes it
//! starts, inherit exactly that filtered set. Revoking takes effect at the next spawn.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::{params, Connection};
use switchyard_schema::command_name;
use switchyard_schema::{Event, Secret, SecretOrigin, SecretSource};

use crate::store::StoredRepository;
use crate::{mcp, policy, repo_policy, Inner};

/// Where secret values live. The real one is [`SystemSecrets`] (the OS keychain and the
/// process environment); tests use a fake and never touch the real keychain.
pub trait SecretValues: Send + Sync {
    /// The keychain's value for `name`, if it holds one.
    fn keychain_get(&self, name: &str) -> Result<Option<String>, String>;
    /// Keep `value` for `name` in the keychain, replacing any earlier value.
    fn keychain_set(&self, name: &str, value: &str) -> Result<(), String>;
    /// `name` in Switchyard's own environment.
    fn environment(&self, name: &str) -> Option<String>;
}

/// No keychain and an empty environment: the default when none is configured.
pub struct NoSecrets;

impl SecretValues for NoSecrets {
    fn keychain_get(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn keychain_set(&self, _: &str, _: &str) -> Result<(), String> {
        Err("no keychain is configured".into())
    }

    fn environment(&self, _: &str) -> Option<String> {
        None
    }
}

/// The OS keychain (generic passwords under the service [`KEYCHAIN_SERVICE`], one per
/// secret name, shared by every repository) and this process's environment.
pub struct SystemSecrets;

pub const KEYCHAIN_SERVICE: &str = "Switchyard secrets";

impl SecretValues for SystemSecrets {
    fn keychain_get(&self, name: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, name).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn keychain_set(&self, name: &str, value: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, name).map_err(|e| e.to_string())?;
        entry.set_password(value).map_err(|e| e.to_string())
    }

    fn environment(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// The environment a spawned agent process gets on top of what it inherits: `set` the
/// granted secrets, `remove` every other known secret name. Launchers apply it with
/// [`SpawnEnv::apply`]; children of the agent (its MCP servers) inherit the result.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SpawnEnv {
    pub set: BTreeMap<String, String>,
    pub remove: BTreeSet<String>,
}

impl SpawnEnv {
    pub fn apply(&self, command: &mut std::process::Command) {
        for name in &self.remove {
            command.env_remove(name);
        }
        for (name, value) in &self.set {
            command.env(name, value);
        }
    }
}

/// Names only: a value never reaches a log through `{:?}`.
impl std::fmt::Debug for SpawnEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnEnv")
            .field("set", &self.set.keys().collect::<Vec<_>>())
            .field("remove", &self.remove)
            .finish()
    }
}

/// An environment variable name: a letter or `_`, then letters, digits or `_`.
pub(crate) fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The repository's secret names and why each is listed, by name. A broken policy or MCP
/// file contributes nothing (sessions refuse to start while it is broken anyway).
fn declared(repo: &StoredRepository) -> BTreeMap<String, SecretOrigin> {
    let primary = Path::new(&repo.path);
    let mut names = BTreeMap::new();
    for name in mcp::load(primary).map(|servers| mcp::secret_refs(&servers)).unwrap_or_default() {
        names.insert(name, SecretOrigin::Mcp);
    }
    for name in repo_policy::load(primary).map(|p| p.secrets).unwrap_or_default() {
        if valid_name(name.trim()) {
            names.insert(name.trim().to_string(), SecretOrigin::Policy);
        }
    }
    names
}

/// The store's `secrets` table: names and flags, never values. A row exists for a name the
/// user added locally (`local`) or ever granted.
pub(crate) fn init(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS secrets (
            repository_id TEXT NOT NULL,
            name          TEXT NOT NULL,
            granted       INTEGER NOT NULL DEFAULT 0,
            local         INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (repository_id, name)
        );",
    )
}

struct Row {
    name: String,
    granted: bool,
    local: bool,
}

fn rows(db: &Connection, repository_id: Option<&str>) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = db.prepare(
        "SELECT name, granted, local FROM secrets WHERE ?1 IS NULL OR repository_id = ?1 ORDER BY name",
    )?;
    let rows = stmt.query_map(params![repository_id], |r| {
        Ok(Row { name: r.get(0)?, granted: r.get(1)?, local: r.get(2)? })
    })?;
    rows.collect()
}

fn set_granted(db: &Connection, repository_id: &str, name: &str, granted: bool) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO secrets (repository_id, name, granted) VALUES (?1, ?2, ?3)
         ON CONFLICT (repository_id, name) DO UPDATE SET granted = excluded.granted",
        params![repository_id, name, granted],
    )
    .map(drop)
}

fn source(inner: &Inner, name: &str) -> SecretSource {
    let values = &inner.runtime.secrets;
    if matches!(values.keychain_get(name), Ok(Some(_))) {
        SecretSource::Keychain
    } else if values.environment(name).is_some() {
        SecretSource::Environment
    } else {
        SecretSource::Missing
    }
}

/// The repository's secrets: declared ones, then the user's local names, with grant flags.
/// A grant on a name the repository no longer lists is not shown and never applies.
fn list(inner: &Inner, repo: &StoredRepository) -> Vec<Secret> {
    let mut names = declared(repo);
    let stored = rows(inner.store.db(), Some(&repo.id)).unwrap_or_default();
    for row in stored.iter().filter(|r| r.local) {
        names.entry(row.name.clone()).or_insert(SecretOrigin::Local);
    }
    names
        .into_iter()
        .map(|(name, origin)| Secret {
            source: source(inner, &name),
            granted: stored.iter().any(|r| r.name == name && r.granted),
            origin,
            name,
        })
        .collect()
}

/// The environment for a spawn in `repo`: its granted secrets that have a value (keychain
/// first, then Switchyard's environment), and every other secret name Switchyard knows
/// (this repository's ungranted ones and every other repository's) stripped.
pub(crate) fn spawn_env(inner: &Inner, repo: &StoredRepository) -> SpawnEnv {
    let values = &inner.runtime.secrets;
    let mut env = SpawnEnv::default();
    for secret in list(inner, repo).into_iter().filter(|s| s.granted) {
        let value = values.keychain_get(&secret.name).ok().flatten().or_else(|| values.environment(&secret.name));
        if let Some(value) = value {
            env.set.insert(secret.name, value);
        }
    }
    let mut known: BTreeSet<String> = rows(inner.store.db(), None).unwrap_or_default().into_iter().map(|r| r.name).collect();
    for other in inner.store.repositories().unwrap_or_default() {
        known.extend(declared(&other).into_keys());
    }
    env.remove = known.into_iter().filter(|name| !env.set.contains_key(name)).collect();
    env
}

/// The `addSecret` command: a local name, not granted.
pub(crate) fn add(inner: &mut Inner, repository_id: &str, name: &str) {
    const COMMAND: &str = command_name::ADD_SECRET;
    let Some(repo) = policy::repository(inner, repository_id) else {
        return inner.refuse(COMMAND, crate::no_repository(&repository_id));
    };
    let name = name.trim();
    if !valid_name(name) {
        return inner.refuse(COMMAND, format!("{name} is not an environment variable name (letters, digits and _)"));
    }
    let saved = inner.store.db().execute(
        "INSERT INTO secrets (repository_id, name, local) VALUES (?1, ?2, 1)
         ON CONFLICT (repository_id, name) DO UPDATE SET local = 1",
        params![repo.id, name],
    );
    if let Err(e) = saved {
        return inner.refuse(COMMAND, format!("cannot save the secret: {e}"));
    }
    emit(inner, &repo);
}

/// The `removeSecret` command: a locally added name and its grant. Declared names stay.
pub(crate) fn remove(inner: &mut Inner, repository_id: &str, name: &str) {
    const COMMAND: &str = command_name::REMOVE_SECRET;
    let Some(repo) = policy::repository(inner, repository_id) else {
        return inner.refuse(COMMAND, crate::no_repository(&repository_id));
    };
    match declared(&repo).get(name) {
        Some(SecretOrigin::Policy) => {
            return inner.refuse(COMMAND, format!("{name} is named in the repository's policy file; change it there"))
        }
        Some(_) => return inner.refuse(COMMAND, format!("{name} is used by the repository's .mcp.json; change it there")),
        None => {}
    }
    let was_granted = list(inner, &repo).iter().any(|s| s.name == name && s.granted);
    match inner.store.db().execute("DELETE FROM secrets WHERE repository_id = ?1 AND name = ?2", params![repo.id, name]) {
        Ok(0) => return inner.refuse(COMMAND, format!("{name} is not one of this repository's secrets")),
        Ok(_) => {}
        Err(e) => return inner.refuse(COMMAND, format!("cannot remove the secret: {e}")),
    }
    emit(inner, &repo);
    if was_granted {
        let note = format!("{name} was revoked for this repository. The running agent keeps it until it is next started.");
        crate::sessions::note_running(inner, &repo.id, &note);
    }
}

/// The `setSecretValue` command: into the keychain, never the store. Not echoed back.
pub(crate) fn set_value(inner: &mut Inner, repository_id: &str, name: &str, value: &str) {
    const COMMAND: &str = command_name::SET_SECRET_VALUE;
    let Some(repo) = policy::repository(inner, repository_id) else {
        return inner.refuse(COMMAND, crate::no_repository(&repository_id));
    };
    if !list(inner, &repo).iter().any(|s| s.name == name) {
        return inner.refuse(COMMAND, format!("{name} is not one of this repository's secrets"));
    }
    if value.is_empty() {
        return inner.refuse(COMMAND, "Type a value first".into());
    }
    if let Err(e) = inner.runtime.secrets.keychain_set(name, value) {
        return inner.refuse(COMMAND, format!("The keychain did not take {name}: {e}"));
    }
    emit(inner, &repo);
}

/// The `grantSecret` and `revokeSecret` commands.
pub(crate) fn set_grant(inner: &mut Inner, repository_id: &str, name: &str, granted: bool) {
    let command = if granted { command_name::GRANT_SECRET } else { command_name::REVOKE_SECRET };
    let Some(repo) = policy::repository(inner, repository_id) else {
        return inner.refuse(command, crate::no_repository(&repository_id));
    };
    let Some(current) = list(inner, &repo).into_iter().find(|s| s.name == name) else {
        return inner.refuse(command, format!("{name} is not one of this repository's secrets"));
    };
    if current.granted == granted {
        return emit(inner, &repo);
    }
    if let Err(e) = set_granted(inner.store.db(), &repo.id, name, granted) {
        return inner.refuse(command, format!("cannot save the grant: {e}"));
    }
    emit(inner, &repo);
    if !granted {
        let note = format!("{name} was revoked for this repository. The running agent keeps it until it is next started.");
        crate::sessions::note_running(inner, &repo.id, &note);
    }
}

fn emit(inner: &Inner, repo: &StoredRepository) {
    inner.emit(Event::SecretsChanged { repository_id: repo.id.clone(), secrets: list(inner, repo) });
}

/// The `readSecrets` command.
pub(crate) fn read(inner: &Inner, repository_id: &str) {
    match policy::repository(inner, repository_id) {
        Some(repo) => emit(inner, &repo),
        None => inner.refuse(command_name::READ_SECRETS, crate::no_repository(&repository_id)),
    }
}
