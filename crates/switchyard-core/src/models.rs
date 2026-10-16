//! The Model-list cache: the Models each agent's CLI offers, per agent and CLI version, with
//! when they were fetched. New session reads it ([`read`]) and never waits on a CLI: the
//! list itself comes from a `list_models` exchange with a running Session's agent, asked
//! when a Session starts and again when New session opens ([`read`]); its answer is
//! stored and pushed ([`answered`]). A CLI upgrade reads its own version's entry, so it
//! starts empty rather than offering the old version's Models.

use rusqlite::{params, Connection, OptionalExtension};
use switchyard_schema::{AgentKind, Event, ListedModel, ModelList};

use crate::Inner;

pub(crate) fn init(db: &Connection) -> rusqlite::Result<()> {
    // `list` is the schema ModelList as JSON (never `empty`).
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS model_lists (
            agent       TEXT NOT NULL,
            cli_version TEXT NOT NULL,
            list        TEXT NOT NULL,
            PRIMARY KEY (agent, cli_version)
        );",
    )
}

fn agent_key(agent: AgentKind) -> String {
    serde_json::to_string(&agent).expect("agent kinds serialize")
}

/// The cached list for `agent`'s CLI at `cli_version`; empty when there is none.
fn cached(db: &Connection, agent: AgentKind, cli_version: &str) -> ModelList {
    let json: Option<String> = db
        .query_row(
            "SELECT list FROM model_lists WHERE agent = ?1 AND cli_version = ?2",
            params![agent_key(agent), cli_version],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);
    json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or(ModelList::Empty)
}

/// What a CLI answered to `list_models`, as the cache keeps it.
pub(crate) fn from_agent(event: adapter_protocol::Event, fetched_at_ms: u64) -> Option<ModelList> {
    match event {
        adapter_protocol::Event::ModelsListed { models } => Some(ModelList::Listed {
            models: models.into_iter().map(|m| ListedModel { id: m.id, display_name: m.display_name }).collect(),
            fetched_at_ms,
        }),
        adapter_protocol::Event::CannotListModels { reason } => Some(ModelList::CannotList { reason }),
        _ => None,
    }
}

/// A Session's CLI (of `agent`, at `cli_version`) answered `list_models`: cache it for that
/// version, and push it when that is the version installed now. A failure never replaces a
/// list the same version gave before (it may only have timed out).
pub(crate) fn answered(inner: &mut Inner, agent: AgentKind, cli_version: Option<&str>, list: ModelList) {
    let Some(version) = cli_version else { return };
    let db = inner.store.db();
    if matches!(list, ModelList::CannotList { .. }) && matches!(cached(db, agent, version), ModelList::Listed { .. }) {
        return;
    }
    let json = serde_json::to_string(&list).expect("model lists serialize");
    let stored = db.execute(
        "INSERT OR REPLACE INTO model_lists (agent, cli_version, list) VALUES (?1, ?2, ?3)",
        params![agent_key(agent), version, json],
    );
    if let Err(e) = stored {
        eprintln!("switchyard: cannot cache the Model list: {e}");
    }
    if crate::compat::installed_version(inner, agent).as_deref() == Some(version) {
        inner.emit(Event::ModelsChanged { agent, models: list });
    }
}

/// The `readModels` command (New session opened): the installed CLI's cached list, at once,
/// then a refresh from a live Session on that version, if there is one.
pub(crate) fn read(inner: &mut Inner, agent: AgentKind) {
    let Some(version) = crate::compat::installed_version(inner, agent) else {
        return inner.emit(Event::ModelsChanged { agent, models: ModelList::Empty });
    };
    let models = cached(inner.store.db(), agent, &version);
    inner.emit(Event::ModelsChanged { agent, models });
    crate::sessions::ask_for_models(inner, agent, &version);
}
