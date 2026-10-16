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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use switchyard_schema::command_name;
use switchyard_schema::{
    AgentKind, ChecklistItem, Event, ModelChoice, PermissionMode, Task, TaskColumn, TaskKind, TaskSessionTarget, TaskSourceKind,
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
    [TaskSourceKind::Local, TaskSourceKind::Github].into_iter().filter_map(|kind| source(inner, repo, kind)).collect()
}

/// The repository's one source of `kind`, if it has one. Only GitHub Issues runs git (to
/// see which branches are linked); local tasks are a file.
fn source<'a>(inner: &'a Inner, repo: &StoredRepository, kind: TaskSourceKind) -> Option<Box<dyn TaskSource + 'a>> {
    let dir = Path::new(&repo.path);
    match kind {
        TaskSourceKind::Local => {
            Some(Box::new(LocalTasks::new(&inner.data_dir, &worktrees::repository_folder(dir))))
        }
        TaskSourceKind::Github => {
            git::remote(dir)?;
            Some(Box::new(GithubIssues {
                db: inner.store.db(),
                repository_id: repo.id.clone(),
                progress: issue_progress(inner, repo),
            }))
        }
    }
}

/// How far the work on each open issue has got, by Switchyard's facts: In progress once a
/// branch is linked to it (a session linked to it, or a worktree branch named for it), In
/// review while that branch's worktree has an open pull request.
fn issue_progress(inner: &Inner, repo: &StoredRepository) -> HashMap<u32, TaskColumn> {
    let db = inner.store.db();
    let issue_of = |task_id: &str| -> Option<u32> {
        let id = TaskId::parse(task_id).filter(|id| id.repository_id == repo.id && id.source == TaskSourceKind::Github)?;
        github::number_of(&id.key)
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

/// A Task's id, `<repository id>/<source>/<key>`: unique across repositories and sources.
/// The key may itself contain `/`; the repository id and source name never do.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskId {
    repository_id: String,
    source: TaskSourceKind,
    key: String,
}

impl TaskId {
    fn new(repo: &StoredRepository, source: TaskSourceKind, key: &str) -> TaskId {
        TaskId { repository_id: repo.id.clone(), source, key: key.to_string() }
    }

    fn parse(id: &str) -> Option<TaskId> {
        let mut parts = id.splitn(3, '/');
        let (repository_id, source, key) = (parts.next()?, parts.next()?, parts.next()?);
        let source = [TaskSourceKind::Local, TaskSourceKind::Github].into_iter().find(|k| source_name(*k) == source)?;
        Some(TaskId { repository_id: repository_id.into(), source, key: key.into() })
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.repository_id, source_name(self.source), self.key)
    }
}

fn task(db: &Connection, repo: &StoredRepository, kind: TaskSourceKind, t: SourceTask) -> Task {
    let id = TaskId::new(repo, kind, &t.key).to_string();
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

/// The repository's whole task list, with when its GitHub issues were last read.
fn tasks_changed(inner: &Inner, repo: &StoredRepository, tasks: Vec<Task>) -> Event {
    Event::TasksChanged {
        repository_id: repo.id.clone(),
        tasks,
        house_rules: house_rules(repo),
        checked_at_ms: github::checked_at(inner.store.db(), &repo.id),
    }
}

/// Emit the repository's whole task list, or refuse `command` with why it cannot be read.
fn announce(inner: &Inner, command: &str, repo: &StoredRepository) {
    match list(inner, repo) {
        Ok(tasks) => inner.emit(tasks_changed(inner, repo, tasks)),
        Err(reason) => inner.refuse(command, format!("Could not read the tasks: {reason}")),
    }
}

/// The repository's local tasks file (which may not exist yet).
pub(crate) fn local_file(inner: &Inner, repo: &StoredRepository) -> PathBuf {
    LocalTasks::new(&inner.data_dir, &worktrees::repository_folder(Path::new(&repo.path))).file().to_path_buf()
}

/// [`Command::ReadTasks`](switchyard_schema::Command::ReadTasks).
pub(crate) fn read(inner: &Inner, repository_id: &str) {
    let Some(repo) = worktrees::repository(inner, repository_id) else {
        return inner.refuse(command_name::READ_TASKS, crate::no_repository(&repository_id));
    };
    announce(inner, command_name::READ_TASKS, &repo);
}

/// [`Command::SyncTasks`](switchyard_schema::Command::SyncTasks): read the repository's
/// issues through the Forge (nothing is written), then announce the board. When the Forge
/// cannot be used the board stays as last synced and the refusal says why.
pub(crate) fn sync(core: &Arc<Mutex<Inner>>, repository_id: &str) {
    const COMMAND: &str = command_name::SYNC_TASKS;
    let (repo, dir, forge) = {
        let inner = core.lock().expect("core lock");
        let Some(repo) = worktrees::repository(&inner, repository_id) else {
            return inner.refuse(COMMAND, crate::no_repository(&repository_id));
        };
        let dir = Path::new(&repo.path).to_path_buf();
        if git::remote(&dir).is_none() {
            announce(&inner, COMMAND, &repo);
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| repo.path.clone());
            return inner.refuse(COMMAND, format!("{name} has no remote, so no GitHub issues; its tasks are local"));
        }
        (repo, dir, inner.runtime.forge.clone())
    };
    // `gh` runs without holding the core.
    let read = forge::admit(core).and_then(|()| forge.issues(&dir));
    let mut inner = core.lock().expect("core lock");
    let failed = match read {
        Ok(issues) => {
            let kept = keep_issues(&inner, &repo.id, &issues);
            // Synced: the Forge poller keeps the issues current from here, starting afresh.
            forge::issues_synced(&mut inner, &repo.id);
            kept.err()
        }
        Err(reason) => Some(reason),
    };
    announce(&inner, COMMAND, &repo);
    if let Some(reason) = failed {
        inner.refuse(COMMAND, format!("Could not sync GitHub issues: {reason}"));
    }
}

/// [`Command::CreateTask`](switchyard_schema::Command::CreateTask): a local task in To do.
pub(crate) fn create(inner: &Inner, repository_id: &str, new: NewTask) {
    const COMMAND: &str = command_name::CREATE_TASK;
    let Some(repo) = worktrees::repository(inner, repository_id) else {
        return inner.refuse(COMMAND, crate::no_repository(&repository_id));
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

/// A task found by id, with its repository: only the task's own source is read.
fn find(inner: &Inner, id: &str) -> Option<(StoredRepository, Task)> {
    let TaskId { repository_id, source: kind, key } = TaskId::parse(id)?;
    let repo = worktrees::repository(inner, &repository_id)?;
    let found = source(inner, &repo, kind)?.tasks().ok()?.into_iter().find(|t| t.key == key)?;
    let found = task(inner.store.db(), &repo, kind, found);
    Some((repo, found))
}

/// [`Command::TickTask`](switchyard_schema::Command::TickTask): kept in the store only.
pub(crate) fn tick(inner: &Inner, task_id: &str, item: u32, done: bool) {
    const COMMAND: &str = command_name::TICK_TASK;
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
    const COMMAND: &str = command_name::LINK_TASK;
    let Some(session) = sessions::live(inner, session_id) else {
        return inner.refuse(COMMAND, crate::no_session(&session_id));
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
        announce(inner, command, repo);
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
    // Only the task's own source is built; one that cannot be written now keeps its
    // column, and the board says so.
    source(inner, repo, found.source).is_some_and(|mut source| source.move_to(&found.key, to, pull_request).is_ok())
}

/// [`Command::StartTaskSession`](switchyard_schema::Command::StartTaskSession).
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_session(
    inner: &mut Inner,
    core: &Arc<Mutex<Inner>>,
    task_id: &str,
    target: TaskSessionTarget,
    agent: AgentKind,
    permission_mode: PermissionMode,
    model: ModelChoice,
    prompt: &str,
) {
    const COMMAND: &str = command_name::START_TASK_SESSION;
    let Some((repo, found)) = find(inner, task_id) else {
        return inner.refuse(COMMAND, format!("There is no task {task_id}"));
    };
    if found.column == TaskColumn::Done {
        return inner.refuse(COMMAND, format!("{} is done", found.key));
    }
    let before = sessions::live_ids(inner);
    match target {
        TaskSessionTarget::NewWorktree { branch } => {
            worktrees::start_session_on_new(inner, core, &repo.id, &branch, agent, permission_mode, model, prompt)
        }
        TaskSessionTarget::Worktree { worktree_path } => {
            sessions::start(inner, core, &worktree_path, agent, permission_mode, model, prompt)
        }
        TaskSessionTarget::Replace { session_id } => {
            sessions::replace(inner, core, &session_id, agent, permission_mode, model, prompt)
        }
    }
    // The session that started (none when it was refused) is linked to the task.
    let started = sessions::live_ids(inner).into_iter().find(|id| !before.contains(id));
    if let Some(session_id) = started {
        attach(inner, COMMAND, &session_id, &repo, &found);
    }
}

/// The tasks of `branch` at `path`: those linked to any session, live or archived, that ran
/// there. A branch's name links nothing.
fn tasks_on(inner: &Inner, path: &str, branch: &str) -> Vec<(StoredRepository, Task)> {
    let db = inner.store.db();
    let mut ids: Vec<String> =
        sessions::ids_on_branch(inner, path, branch).iter().filter_map(|s| linked_task(db, s)).collect();
    ids.sort();
    ids.dedup();
    ids.iter().filter_map(|id| find(inner, id)).collect()
}

/// Move the tasks of `branch` at `path` to `to`, announcing each changed board once.
/// `command`: what moved them, to name if the board cannot be read; none for the poller,
/// which then leaves the board as it was without a word.
fn advance_all(
    inner: &Inner,
    command: Option<&str>,
    tasks: Vec<(StoredRepository, Task)>,
    to: TaskColumn,
    pull_request: Option<&str>,
) {
    let mut changed: Vec<StoredRepository> = Vec::new();
    for (repo, found) in tasks {
        if advance(inner, &repo, &found, to, pull_request) && !changed.iter().any(|r| r.id == repo.id) {
            changed.push(repo);
        }
    }
    for repo in changed {
        match command {
            Some(command) => announce(inner, command, &repo),
            None => {
                if let Ok(tasks) = list(inner, &repo) {
                    inner.emit(tasks_changed(inner, &repo, tasks));
                }
            }
        }
    }
}

/// The worktree's pull request is open (opened from Switchyard, or found by the poller):
/// its tasks move to In review.
pub(crate) fn pull_request_open(inner: &Inner, command: Option<&str>, path: &str, branch: &str) {
    advance_all(inner, command, tasks_on(inner, path, branch), TaskColumn::InReview, None);
}

/// The Forge reports the worktree's branch Merged by `pull_request` (its address), or a
/// Local merge finished: its tasks move to Done. Call it before the worktree is removed,
/// so the branch's issue is still found.
pub(crate) fn merged(inner: &Inner, command: Option<&str>, path: &str, branch: &str, pull_request: Option<&str>) {
    advance_all(inner, command, tasks_on(inner, path, branch), TaskColumn::Done, pull_request);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_id_reads_back_what_it_writes_and_keeps_slashes_in_the_key() {
        let repo = StoredRepository { id: "repo-3".into(), path: "/src/acme".into() };
        let id = TaskId::new(&repo, TaskSourceKind::Github, "#412");
        assert_eq!(id.to_string(), "repo-3/github/#412");
        assert_eq!(TaskId::parse("repo-3/github/#412"), Some(id));
        let odd = TaskId::parse("repo-3/local/L-1/x").unwrap();
        assert_eq!((odd.source, odd.key.as_str()), (TaskSourceKind::Local, "L-1/x"));
        assert_eq!(TaskId::parse("repo-3/jira/X-1"), None);
        assert_eq!(TaskId::parse("repo-3"), None);
    }
}

/// The GitHub issue numbers linked to the sessions that ran on `branch` at `path`: what a
/// pull request opened from there closes on merge.
pub(crate) fn linked_issues(inner: &Inner, path: &str, branch: &str) -> Vec<u32> {
    let db = inner.store.db();
    let mut issues: Vec<u32> = sessions::ids_on_branch(inner, path, branch)
        .iter()
        .filter_map(|s| linked_task(db, s))
        .filter_map(|t| TaskId::parse(&t))
        .filter(|id| id.source == TaskSourceKind::Github)
        .filter_map(|id| github::number_of(&id.key))
        .collect();
    issues.sort_unstable();
    issues.dedup();
    issues
}

/// Read a repository's issues again (on the Forge poller's schedule, or after a merge) and
/// announce the board with the time they were read; quiet when the Forge cannot answer.
/// Runs `gh` without holding the core. Only for a repository that has been synced before.
pub(crate) fn refresh_issues(core: &Arc<Mutex<Inner>>, repository_id: &str) {
    let (dir, forge) = {
        let inner = core.lock().expect("core lock");
        let Some(repo) = worktrees::repository(&inner, repository_id) else { return };
        if !github::synced(inner.store.db(), repository_id) {
            return;
        }
        (Path::new(&repo.path).to_path_buf(), inner.runtime.forge.clone())
    };
    let read = forge.issues(&dir);
    let mut inner = core.lock().expect("core lock");
    let Ok(issues) = read else {
        // Offline, say: try again later.
        return forge::issues_polled(&mut inner, repository_id, false);
    };
    let Some(repo) = worktrees::repository(&inner, repository_id) else { return };
    let changed = github::differ(inner.store.db(), &repo.id, &issues);
    forge::issues_polled(&mut inner, repository_id, changed);
    if keep_issues(&inner, &repo.id, &issues).is_ok() {
        if let Ok(tasks) = list(&inner, &repo) {
            inner.emit(tasks_changed(&inner, &repo, tasks));
        }
    }
}

/// Store `issues` as the repository's GitHub issues, read just now.
fn keep_issues(inner: &Inner, repository_id: &str, issues: &[crate::forge::ForgeIssue]) -> Result<(), String> {
    let now = (inner.runtime.now_ms)();
    github::save(inner.store.db(), repository_id, issues).and_then(|()| github::checked(inner.store.db(), repository_id, now))
}

/// Is the repository's GitHub issues source one the Forge poller keeps current (synced
/// at least once)?
pub(crate) fn issues_synced(inner: &Inner, repository_id: &str) -> bool {
    github::synced(inner.store.db(), repository_id)
}
