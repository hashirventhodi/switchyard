//! Switchyard core. Contains no Tauri types (ADR-0001).
//!
//! The only way in is [`Core::dispatch`] with a schema [`Command`]; the only way out is the
//! [`Event`] channel handed to [`Core::open`]. The Tauri app, tests and any future
//! standalone binary all drive the core through that pair. Agents run through the
//! [`agents::AgentLauncher`] in the [`Runtime`]; their output re-enters the core on the
//! launcher's threads and leaves as events on the same channel.

pub mod agents;
pub mod compat;
mod git;
mod rebase;
mod policy;
mod repo_policy;
mod review;
mod sessions;
mod shells;
pub mod sidecar;
mod spend;
mod store;
mod worktrees;

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use switchyard_schema::{Command, Event, Repository, Worktree};

use agents::{AgentLauncher, NoAgents};
use compat::{AgentProbe, Compat, NoProbe};
use sessions::Sessions;
use shells::{ShellProgram, Shells};
use store::{Store, StoredRepository};

/// What the core runs on besides its data directory.
pub struct Runtime {
    pub agents: Arc<dyn AgentLauncher>,
    /// Wall clock, milliseconds since the Unix epoch.
    pub now_ms: fn() -> u64,
    /// The Switchyard-owned directory new worktrees go under, outside every repository
    /// (one subdirectory per repository, keyed by a hash of its path). A repository's
    /// policy may name its own root instead.
    pub worktree_root: PathBuf,
    /// Detects installed agent CLIs and asks adapters for their manifests.
    pub probe: Arc<dyn AgentProbe>,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime { agents: Arc::new(NoAgents), now_ms: system_now_ms, probe: Arc::new(NoProbe), worktree_root: default_worktree_root() }
    }
}

/// `~/.switchyard/worktrees`: short, ASCII, and outside every repository.
pub fn default_worktree_root() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join(".switchyard").join("worktrees")
}

pub fn system_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct Core {
    inner: Arc<Mutex<Inner>>,
}

pub(crate) struct Inner {
    data_dir: PathBuf,
    events: Sender<Event>,
    store: Store,
    runtime: Runtime,
    sessions: Sessions,
    shells: Shells,
    compat: Compat,
    review: review::Decisions,
}

#[derive(Debug)]
pub struct CoreError(pub String);

impl Core {
    /// Open the core on `data_dir` (created if missing), emitting events into `events`.
    /// No agents can be started; see [`Core::open_with`].
    pub fn open(data_dir: &Path, events: Sender<Event>) -> Result<Core, CoreError> {
        Self::open_with(data_dir, events, Runtime::default())
    }

    pub fn open_with(data_dir: &Path, events: Sender<Event>, runtime: Runtime) -> Result<Core, CoreError> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| CoreError(format!("cannot create {}: {e}", data_dir.display())))?;
        let store = Store::open(data_dir).map_err(|e| CoreError(format!("cannot open store: {e}")))?;
        let compat = Compat::open(data_dir);
        let inner = Inner {
            data_dir: data_dir.to_path_buf(),
            events,
            store,
            runtime,
            sessions: Sessions::default(),
            shells: Shells::default(),
            compat,
            review: Default::default(),
        };
        Ok(Core { inner: Arc::new(Mutex::new(inner)) })
    }

    /// Run `program` with `args` in shell terminals instead of the user's login shell
    /// (`$SHELL -l`). Tests use a shell with no rc files.
    pub fn set_shell(&mut self, program: PathBuf, args: Vec<String>) {
        self.inner.lock().expect("core lock").shells.set_program(ShellProgram { program, args });
    }

    pub fn dispatch(&mut self, command: Command) {
        let mut inner = self.inner.lock().expect("core lock");
        match command {
            Command::Attach => inner.attach(),
            Command::AddRepository { path } => inner.add_repository(&path),
            Command::StartSession { worktree_path, agent, permission_mode, prompt } => {
                sessions::start(&mut inner, &self.inner, &worktree_path, agent, permission_mode, &prompt)
            }
            Command::StopSession { session_id } => sessions::stop(&mut inner, &session_id),
            Command::SendPrompt { session_id, text } => sessions::send_prompt(&mut inner, &session_id, &text),
            Command::SetPermissionMode { session_id, permission_mode } => {
                sessions::set_mode(&mut inner, &session_id, permission_mode)
            }
            Command::OpenShell { worktree_path, cols, rows } => {
                shells::open(&mut inner, &self.inner, &worktree_path, cols, rows)
            }
            Command::WriteShell { shell_id, data } => shells::write(&mut inner, &shell_id, &data),
            Command::ResizeShell { shell_id, cols, rows } => shells::resize(&mut inner, &shell_id, cols, rows),
            Command::CloseShell { shell_id } => shells::close(&mut inner, &shell_id),
            Command::CloseWorktreeShells { worktree_path } => shells::close_worktree(&mut inner, &worktree_path),
            Command::CreateWorktree { repository_id, branch } => {
                worktrees::create(&mut inner, "createWorktree", &repository_id, &branch);
            }
            Command::StartSessionOnNewWorktree { repository_id, branch, agent, permission_mode, prompt } => {
                worktrees::start_session_on_new(&mut inner, &self.inner, &repository_id, &branch, agent, permission_mode, &prompt)
            }
            Command::RemoveWorktree { worktree_path } => worktrees::remove(&mut inner, &worktree_path),
            Command::RefreshWorktrees => worktrees::refresh(&inner),
            Command::AnswerRequest { session_id, request_id, verdict } => {
                sessions::answer_request(&mut inner, &session_id, &request_id, verdict)
            }
            Command::CheckAgents => compat::check(&mut inner),
            Command::ReadChanges { worktree_path } => review::read(&mut inner, &worktree_path),
            Command::DecideFile { worktree_path, path, decision } => {
                review::decide(&mut inner, &worktree_path, &path, decision)
            }
            Command::CommitAccepted { worktree_path, message } => review::commit(&mut inner, &worktree_path, &message),
            Command::ReadPolicy { repository_id } => policy::read(&inner, &repository_id),
            Command::RevokeGrant { repository_id, grant_id } => policy::revoke(&inner, &repository_id, &grant_id),
            Command::RebaseWorktree { worktree_path } => rebase::rebase(&mut inner, &worktree_path),
            Command::AskAgentToRebase { session_id } => rebase::ask_agent(&mut inner, &session_id),
            Command::ReadHistory { worktree_path } => rebase::history(&inner, &worktree_path),
        }
    }
}

impl Inner {
    pub(crate) fn emit(&self, event: Event) {
        // A closed channel means nobody is listening; the core carries on regardless.
        let _ = self.events.send(event);
    }

    pub(crate) fn refuse(&self, command: &str, reason: String) {
        self.emit(Event::CommandRefused { command: command.into(), reason });
    }

    fn attach(&mut self) {
        self.emit(Event::Attached);
        compat::replay(self);
        let stored = match self.store.repositories() {
            Ok(list) => list,
            Err(e) => return self.refuse("attach", format!("cannot read the store: {e}")),
        };
        for repo in stored {
            if let Err(reason) = self.announce(&repo) {
                self.refuse("attach", reason);
            }
        }
        sessions::replay(self);
        shells::replay(self);
    }

    fn add_repository(&mut self, path: &str) {
        const COMMAND: &str = "addRepository";
        let dir = Path::new(path);
        if !dir.is_dir() {
            return self.refuse(COMMAND, format!("{path} does not exist or is not a folder"));
        }
        if !git::is_inside_repository(dir) {
            return self.refuse(COMMAND, format!("{path} is not a git repository"));
        }
        let primary = match git::worktrees(dir).map(|list| list.into_iter().next()) {
            Ok(Some(primary)) => primary.path.to_string_lossy().into_owned(),
            Ok(None) => return self.refuse(COMMAND, format!("{path} is a bare repository")),
            Err(git::GitError(e)) => return self.refuse(COMMAND, format!("git could not read {path}: {e}")),
        };
        let stored = match self.store.repository_by_path(&primary) {
            Ok(Some(_)) => return self.refuse(COMMAND, format!("{primary} is already added")),
            Ok(None) => self.store.insert_repository(&primary),
            Err(e) => Err(e),
        };
        match stored {
            Ok(repo) => {
                if let Err(reason) = self.announce(&repo) {
                    self.refuse(COMMAND, reason);
                }
            }
            Err(e) => self.refuse(COMMAND, format!("cannot save the repository: {e}")),
        }
    }

    /// Read a stored repository's facts from git and emit them.
    fn announce(&self, repo: &StoredRepository) -> Result<(), String> {
        let dir = Path::new(&repo.path);
        let worktrees = match git::worktrees(dir) {
            Ok(list) if !list.is_empty() => list,
            _ => return Err(format!("{} is no longer a readable git repository", repo.path)),
        };
        let primary = &worktrees[0];
        let repository = Repository {
            id: repo.id.clone(),
            name: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            path: repo.path.clone(),
            base_branch: git::base_branch(dir, primary),
        };
        self.emit(Event::RepositoryAdded { repository });
        for wt in worktrees {
            self.emit(Event::WorktreeAdded {
                repository_id: repo.id.clone(),
                worktree: Worktree {
                    path: wt.path.to_string_lossy().into_owned(),
                    branch: wt.branch,
                    primary: wt.primary,
                },
            });
        }
        Ok(())
    }

    /// The stored repository and git's view of the worktree at `path`, if it is one of
    /// a known repository's worktrees. Re-read from git every time.
    pub(crate) fn find_worktree(&self, path: &str) -> Option<(StoredRepository, git::GitWorktree)> {
        let repos = self.store.repositories().ok()?;
        repos.into_iter().find_map(|repo| {
            let list = git::worktrees(Path::new(&repo.path)).ok()?;
            let wt = list.into_iter().find(|w| w.path == Path::new(path))?;
            Some((repo, wt))
        })
    }
}
