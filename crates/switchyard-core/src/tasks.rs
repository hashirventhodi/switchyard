//! Tasks: the reason a Session exists. Each Repository's Tasks come from its TaskSources
//! ([`TaskSource`]: local tasks today, GitHub Issues next) and sit in one column of the
//! board. A Session links to at most one Task; relinking is explicit.
//!
//! A task moves only forward, and only on the events the core already acts on: a session
//! starting on it (or linked to it) moves it to In progress, its worktree's pull request
//! opening to In review ([`pull_request_open`], from the Forge), the Forge reporting
//! Merged to Done ([`merged`]). The move is written back to the task's source, which is
//! the one place a task's column lives; nothing else keeps a second copy.
//!
//! What is Switchyard's own lives in the store, never in a source: which Session links to
//! which Task, and the ticks on each task's checklist.

mod github;
mod local;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use switchyard_schema::{
    AgentKind, ChecklistItem, Event, PermissionMode, Task, TaskColumn, TaskKind, TaskSessionTarget, TaskSourceKind,
};

use crate::store::StoredRepository;
use crate::{forge, git, repo_policy, sessions, worktrees, Inner};

use github::GithubIssues;
use local::LocalTasks;

/// What a session started for a Task is told besides the task, when the Repository's
/// policy names no `house_rules`.
pub const DEFAULT_HOUSE_RULES: &str =
    "Write a failing test first, keep changes minimal, and stop to ask before running migrations or destructive commands.";

/// Where a Repository's Tasks come from. Each source owns its tasks' columns: the core
/// asks it to move one ([`TaskSource::move_to`]) and reads the result back.
pub(crate) trait TaskSource {
    fn kind(&self) -> TaskSourceKind;
    /// Every task the source has for the repository, in the source's order.
    fn tasks(&self) -> Result<Vec<SourceTask>, String>;
    /// Write a column change back (only ever forward). `pull_request` is the address of the
    /// pull request that moved it, when there is one.
    fn move_to(&mut self, key: &str, column: TaskColumn, pull_request: Option<&str>) -> Result<(), String>;
    /// Add a task to To do. Sources that only mirror a tracker refuse.
    fn create(&mut self, _new: NewTask) -> Result<SourceTask, String> {
        Err("tasks are created on the tracker, not in Switchyard".into())
    }
}

/// A task as its source has it; the core adds the id, repository and checklist ticks.
pub(crate) struct SourceTask {
    /// Unique within the source (`L-3`, `#412`).
    pub key: String,
    pub title: String,
    pub description: String,
    pub kind: TaskKind,
    pub column: TaskColumn,
    /// The checklist items' text (ticks are the store's).
    pub checklist: Vec<String>,
    pub url: Option<String>,
}

pub(crate) struct NewTask {
    pub title: String,
    pub description: String,
    pub kind: TaskKind,
    pub checklist: Vec<String>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS task_links (
    session_id TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS task_checks (
    task_id TEXT NOT NULL,
    item    TEXT NOT NULL,
    PRIMARY KEY (task_id, item)
);
";

/// Create the tables tasks keep in the store (called when the store opens).
pub(crate) fn init(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(SCHEMA)?;
    github::init(db)
}

/// The TaskSources of a repository: its local tasks, and its GitHub Issues when it has a
/// remote.
fn sources<'a>(inner: &'a Inner, repo: &StoredRepository) -> Vec<Box<dyn TaskSource + 'a>> {
    let dir = Path::new(&repo.path);
    let folder = worktrees::repository_folder(dir);
    let mut list: Vec<Box<dyn TaskSource + 'a>> = vec![Box::new(LocalTasks::new(&inner.data_dir, &folder))];
    if git::remote(dir).is_some() {
        list.push(Box::new(GithubIssues {
            db: inner.store.db(),
            forge: inner.runtime.forge.clone(),
            repository_id: repo.id.clone(),
            dir: dir.to_path_buf(),
            progress: issue_progress(inner, repo),
        }));
    }
    list
}

/// How far the work on each open issue has got, by Switchyard's facts: In progress once a
/// branch is linked to it (a session linked to it, or a worktree branch named for it), In
/// review while that branch's worktree has an open pull request.
fn issue_progress(inner: &Inner, repo: &StoredRepository) -> HashMap<u32, TaskColumn> {
    let db = inner.store.db();
    let issue_of = |task_id: &str| -> Option<u32> {
        let key = task_id.strip_prefix(&format!("{}/github/", repo.id))?;
        github::number_of(key)
    };
    let mut progress = HashMap::new();
    let mut reach = |n: u32, column: TaskColumn| {
        let at = progress.entry(n).or_insert(column);
        *at = (*at).max(column);
    };
    let read = || -> rusqlite::Result<Vec<String>> {
        let mut stmt = db.prepare("SELECT task_id FROM task_links")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    };
    for n in read().unwrap_or_default().iter().filter_map(|t| issue_of(t)) {
        reach(n, TaskColumn::InProgress);
    }
    for wt in git::worktrees(Path::new(&repo.path)).unwrap_or_default().into_iter().filter(|w| !w.primary) {
        let Some(branch) = wt.branch else { continue };
        let path = wt.path.to_string_lossy().into_owned();
        let linked: Vec<u32> = sessions::ids_on_branch(inner, &path, &branch)
            .iter()
            .filter_map(|s| linked_task(db, s))
            .filter_map(|t| issue_of(&t))
            .chain(github::named_issue(&branch))
            .collect();
        let column =
            if forge::has_open_pull_request(inner, &path) { TaskColumn::InReview } else { TaskColumn::InProgress };
        for n in linked {
            reach(n, column);
        }
    }
    progress
}

fn source_name(kind: TaskSourceKind) -> &'static str {
    match kind {
        TaskSourceKind::Local => "local",
        TaskSourceKind::Github => "github",
    }
}

/// `<repository id>/<source>/<key>`: unique across repositories and sources.
fn task_id(repo: &StoredRepository, kind: TaskSourceKind, key: &str) -> String {
    format!("{}/{}/{key}", repo.id, source_name(kind))
}

fn task(db: &Connection, repo: &StoredRepository, kind: TaskSourceKind, t: SourceTask) -> Task {
    let id = task_id(repo, kind, &t.key);
    let ticked = ticks(db, &id);
    Task {
        checklist: t
            .checklist
            .into_iter()
            .map(|text| ChecklistItem { done: ticked.contains(&text), text })
            .collect(),
        id,
        repository_id: repo.id.clone(),
        source: kind,
        key: t.key,
        title: t.title,
        description: t.description,
        kind: t.kind,
        column: t.column,
        url: t.url,
    }
}

fn ticks(db: &Connection, task_id: &str) -> Vec<String> {
    let read = || -> rusqlite::Result<Vec<String>> {
        let mut stmt = db.prepare("SELECT item FROM task_checks WHERE task_id = ?1")?;
        let rows = stmt.query_map(params![task_id], |r| r.get(0))?;
        rows.collect()
    };
    read().unwrap_or_default()
}

/// Every task of the repository, from every source.
fn list(inner: &Inner, repo: &StoredRepository) -> Result<Vec<Task>, String> {
    let mut out = Vec::new();
    for source in sources(inner, repo) {
        let kind = source.kind();
        for t in source.tasks()? {
            out.push(task(inner.store.db(), repo, kind, t));
        }
    }
    Ok(out)
}

fn house_rules(repo: &StoredRepository) -> String {
    repo_policy::load(Path::new(&repo.path))
        .ok()
        .and_then(|p| p.house_rules)
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| DEFAULT_HOUSE_RULES.to_string())
}

/// Emit the repository's whole task list, or refuse `command` with why it cannot be read.
fn announce(inner: &Inner, command: &str, repo: &StoredRepository) {
    match list(inner, repo) {
        Ok(tasks) => inner.emit(Event::TasksChanged {
            repository_id: repo.id.clone(),
            tasks,
            house_rules: house_rules(repo),
        }),
        Err(reason) => inner.refuse(command, format!("Could not read the tasks: {reason}")),
    }
}

/// [`Command::ReadTasks`](switchyard_schema::Command::ReadTasks).
pub(crate) fn read(inner: &Inner, repository_id: &str) {
    let Some(repo) = worktrees::repository(inner, repository_id) else {
        return inner.refuse("readTasks", format!("There is no repository {repository_id}"));
    };
    announce(inner, "readTasks", &repo);
}

/// [`Command::SyncTasks`](switchyard_schema::Command::SyncTasks): read the repository's
/// issues through the Forge (nothing is written), then announce the board. When the Forge
/// cannot be used the board stays as last synced and the refusal says why.
pub(crate) fn sync(inner: &mut Inner, repository_id: &str) {
    const COMMAND: &str = "syncTasks";
    let Some(repo) = worktrees::repository(inner, repository_id) else {
        return inner.refuse(COMMAND, format!("There is no repository {repository_id}"));
    };
    let dir = Path::new(&repo.path).to_path_buf();
    if git::remote(&dir).is_none() {
        announce(inner, COMMAND, &repo);
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| repo.path.clone());
        return inner.refuse(COMMAND, format!("{name} has no remote, so no GitHub issues; its tasks are local"));
    }
    let read = forge::admit(inner).and_then(|()| inner.runtime.forge.issues(&dir));
    let failed = match read {
        Ok(issues) => github::save(inner.store.db(), &repo.id, &issues).err(),
        Err(reason) => Some(reason),
    };
    announce(inner, COMMAND, &repo);
    if let Some(reason) = failed {
        inner.refuse(COMMAND, format!("Could not sync GitHub issues: {reason}"));
    }
}

/// [`Command::CreateTask`](switchyard_schema::Command::CreateTask): a local task in To do.
pub(crate) fn create(inner: &Inner, repository_id: &str, new: NewTask) {
    const COMMAND: &str = "createTask";
    let Some(repo) = worktrees::repository(inner, repository_id) else {
        return inner.refuse(COMMAND, format!("There is no repository {repository_id}"));
    };
    let title = new.title.trim().to_string();
    if title.is_empty() {
        return inner.refuse(COMMAND, "Give the task a title".into());
    }
    let checklist = new.checklist.iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
    let new = NewTask { title, description: new.description.trim().to_string(), kind: new.kind, checklist };
    let folder = worktrees::repository_folder(Path::new(&repo.path));
    let mut local = LocalTasks::new(&inner.data_dir, &folder);
    match local.create(new) {
        Ok(created) => {
            let created = task(inner.store.db(), &repo, TaskSourceKind::Local, created);
            inner.emit(Event::TaskCreated { task: created });
            announce(inner, COMMAND, &repo);
        }
        Err(reason) => inner.refuse(COMMAND, format!("Could not add the task: {reason}")),
    }
}

/// A task found by id, with its repository.
fn find(inner: &Inner, id: &str) -> Option<(StoredRepository, Task)> {
    let repo_id = id.split('/').next()?;
    let repo = worktrees::repository(inner, repo_id)?;
    let found = list(inner, &repo).ok()?.into_iter().find(|t| t.id == id)?;
    Some((repo, found))
}

/// [`Command::TickTask`](switchyard_schema::Command::TickTask): kept in the store only.
pub(crate) fn tick(inner: &Inner, task_id: &str, item: u32, done: bool) {
    const COMMAND: &str = "tickTask";
    let Some((repo, found)) = find(inner, task_id) else {
        return inner.refuse(COMMAND, format!("There is no task {task_id}"));
    };
    let Some(entry) = found.checklist.get(item as usize) else {
        return inner.refuse(COMMAND, format!("{} has no checklist item {}", found.key, item + 1));
    };
    let db = inner.store.db();
    let written = if done {
        db.execute("INSERT OR IGNORE INTO task_checks (task_id, item) VALUES (?1, ?2)", params![task_id, entry.text])
    } else {
        db.execute("DELETE FROM task_checks WHERE task_id = ?1 AND item = ?2", params![task_id, entry.text])
    };
    if let Err(e) = written {
        return inner.refuse(COMMAND, format!("Could not save the checklist: {e}"));
    }
    announce(inner, COMMAND, &repo);
}

fn linked_task(db: &Connection, session_id: &str) -> Option<String> {
    db.query_row("SELECT task_id FROM task_links WHERE session_id = ?1", params![session_id], |r| r.get(0)).ok()
}

/// [`Command::LinkTask`](switchyard_schema::Command::LinkTask).
pub(crate) fn link(inner: &Inner, session_id: &str, task_id: Option<&str>, relink: bool) {
    const COMMAND: &str = "linkTask";
    let Some(session) = sessions::live(inner, session_id) else {
        return inner.refuse(COMMAND, format!("There is no session {session_id}"));
    };
    let db = inner.store.db();
    let Some(task_id) = task_id else {
        if let Err(e) = db.execute("DELETE FROM task_links WHERE session_id = ?1", params![session_id]) {
            return inner.refuse(COMMAND, format!("Could not unlink the task: {e}"));
        }
        return inner.emit(Event::TaskLinked { session_id: session_id.into(), task_id: None });
    };
    let Some((repo, found)) = find(inner, task_id) else {
        return inner.refuse(COMMAND, format!("There is no task {task_id}"));
    };
    if let Some(current) = linked_task(db, session_id).filter(|c| c != task_id) {
        if !relink {
            let name = session.branch.clone().unwrap_or(session.worktree_path.clone());
            let key = find(inner, &current).map(|(_, t)| t.key).unwrap_or(current);
            return inner.refuse(COMMAND, format!("{name} is already linked to {key}; relink it to change that"));
        }
    }
    attach(inner, COMMAND, session_id, &repo, &found);
}

/// Record the link, announce it, and move the task to In progress.
fn attach(inner: &Inner, command: &str, session_id: &str, repo: &StoredRepository, found: &Task) {
    let saved = inner.store.db().execute(
        "INSERT INTO task_links (session_id, task_id) VALUES (?1, ?2)
         ON CONFLICT(session_id) DO UPDATE SET task_id = excluded.task_id",
        params![session_id, found.id],
    );
    if let Err(e) = saved {
        return inner.refuse(command, format!("Could not link the task: {e}"));
    }
    inner.emit(Event::TaskLinked { session_id: session_id.into(), task_id: Some(found.id.clone()) });
    if advance(inner, repo, found, TaskColumn::InProgress, None) {
        announce(inner, "moveTask", repo);
    }
}

/// Move a task forward to `to` through its source (`pull_request`: the address of the pull
/// request that moved it). True when the board changed and should be announced.
fn advance(inner: &Inner, repo: &StoredRepository, found: &Task, to: TaskColumn, pull_request: Option<&str>) -> bool {
    if found.column >= to {
        // An issue's In progress and In review follow Switchyard's own facts, so the event
        // that moves it there already shows when it is read: announce it all the same.
        return found.source == TaskSourceKind::Github && found.column == to && to != TaskColumn::Done;
    }
    let mut moved = false;
    for mut source in sources(inner, repo) {
        if source.kind() == found.source {
            // A source that cannot be written now keeps its column; the board says so.
            moved = source.move_to(&found.key, to, pull_request).is_ok();
        }
    }
    moved
}

/// [`Command::StartTaskSession`](switchyard_schema::Command::StartTaskSession).
pub(crate) fn start_session(
    inner: &mut Inner,
    core: &Arc<Mutex<Inner>>,
    task_id: &str,
    target: TaskSessionTarget,
    agent: AgentKind,
    permission_mode: PermissionMode,
    prompt: &str,
) {
    const COMMAND: &str = "startTaskSession";
    let Some((repo, found)) = find(inner, task_id) else {
        return inner.refuse(COMMAND, format!("There is no task {task_id}"));
    };
    if found.column == TaskColumn::Done {
        return inner.refuse(COMMAND, format!("{} is done", found.key));
    }
    let before = sessions::live_ids(inner);
    match target {
        TaskSessionTarget::NewWorktree { branch } => {
            worktrees::start_session_on_new(inner, core, &repo.id, &branch, agent, permission_mode, prompt)
        }
        TaskSessionTarget::Worktree { worktree_path } => {
            sessions::start(inner, core, &worktree_path, agent, permission_mode, prompt)
        }
        TaskSessionTarget::Replace { session_id } => {
            sessions::replace(inner, core, &session_id, agent, permission_mode, prompt)
        }
    }
    // The session that started (none when it was refused) is linked to the task.
    let started = sessions::live_ids(inner).into_iter().find(|id| !before.contains(id));
    if let Some(session_id) = started {
        attach(inner, COMMAND, &session_id, &repo, &found);
    }
}

/// The tasks of `branch` at `path`: those linked to any session, live or archived, that ran
/// there, and the issue the branch is named for (`fix/412-…`), as GitHub links them.
fn tasks_on(inner: &Inner, path: &str, branch: &str) -> Vec<(StoredRepository, Task)> {
    let db = inner.store.db();
    let mut ids: Vec<String> =
        sessions::ids_on_branch(inner, path, branch).iter().filter_map(|s| linked_task(db, s)).collect();
    if let (Some(n), Some((repo, _))) = (github::named_issue(branch), inner.find_worktree(path)) {
        ids.push(task_id(&repo, TaskSourceKind::Github, &format!("#{n}")));
    }
    ids.sort();
    ids.dedup();
    ids.iter().filter_map(|id| find(inner, id)).collect()
}

/// Move the tasks of `branch` at `path` to `to`, announcing each changed board once.
fn advance_all(inner: &Inner, tasks: Vec<(StoredRepository, Task)>, to: TaskColumn, pull_request: Option<&str>) {
    let mut changed: Vec<StoredRepository> = Vec::new();
    for (repo, found) in tasks {
        if advance(inner, &repo, &found, to, pull_request) && !changed.iter().any(|r| r.id == repo.id) {
            changed.push(repo);
        }
    }
    for repo in changed {
        announce(inner, "moveTask", &repo);
    }
}

/// The worktree's pull request is open (opened from Switchyard, or found by the poller):
/// its tasks move to In review.
pub(crate) fn pull_request_open(inner: &Inner, path: &str, branch: &str) {
    advance_all(inner, tasks_on(inner, path, branch), TaskColumn::InReview, None);
}

/// The Forge reports the worktree's branch Merged by `pull_request` (its address), or a
/// Local merge finished: its tasks move to Done. Call it before the worktree is removed,
/// so the branch's issue is still found.
pub(crate) fn merged(inner: &Inner, path: &str, branch: &str, pull_request: Option<&str>) {
    advance_all(inner, tasks_on(inner, path, branch), TaskColumn::Done, pull_request);
}

/// On attach: every link of a session the core still knows (live or archived).
pub(crate) fn replay_links(inner: &Inner) {
    let read = || -> rusqlite::Result<Vec<(String, String)>> {
        let mut stmt = inner.store.db().prepare("SELECT session_id, task_id FROM task_links ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    };
    for (session_id, task_id) in read().unwrap_or_default() {
        if sessions::known(inner, &session_id) {
            inner.emit(Event::TaskLinked { session_id, task_id: Some(task_id) });
        }
    }
}
