//! Switchyard core. Contains no Tauri types (ADR-0001).
//!
//! The only way in is [`Core::dispatch`] with a schema [`Command`]; the only way out is the
//! [`Event`] channel handed to [`Core::open`]. The Tauri app, tests and any future
//! standalone binary all drive the core through that pair. Agents run through the
//! [`agents::AgentLauncher`] in the [`Runtime`]; their output re-enters the core on the
//! launcher's threads and leaves as events on the same channel.

mod acknowledgements;
pub mod agents;
pub mod codex;
pub mod compat;
pub mod context;
pub mod forge;
mod git;
pub mod mcp;
pub mod processes;
mod rebase;
mod policy;
mod repo_policy;
mod session_store;
mod review;
pub mod secrets;
mod sessions;
mod shells;
pub mod sidecar;
mod spend;
mod store;
mod subprocess;
mod tasks;
pub mod watcher;
mod disk;
mod worktrees;

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, Weak};

use switchyard_schema::command_name;
use switchyard_schema::{Command, Event, Repository, Worktree};

use agents::{AgentLauncher, NoAgents};
use compat::{AgentProbe, Compat, NoProbe};
use forge::{Forge, NoForge};
use processes::{ProcessTable, SystemProcesses};
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
    /// The machine's processes: start times of recorded pids, and killing orphans.
    pub processes: Arc<dyn ProcessTable>,
    /// Where "Export transcript" writes its JSONL files.
    pub export_dir: PathBuf,
    /// Where secret values live: the OS keychain and the environment.
    pub secrets: Arc<dyn secrets::SecretValues>,
    /// Where pull requests are opened and polled (GitHub through `gh`).
    pub forge: Arc<dyn Forge>,
    /// How often the core polls the Forge on its own; `None`: only when
    /// [`Core::poll_forge`] is called (tests tick it by hand).
    pub forge_poll_every: Option<std::time::Duration>,
    /// What supports a repository's sandbox claim on this machine (a container or VM
    /// Switchyard runs in), shown beside the request; `None`: nothing to show.
    pub sandbox_evidence: fn() -> Option<String>,
    /// Claude Code's home (`~/.claude`): where it keeps each project's memory, read for
    /// Context.
    pub claude_home: PathBuf,
    /// Tells the core when a Worktree changes on disk (ADR-0008).
    pub watcher: Arc<dyn watcher::Watcher>,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime {
            agents: Arc::new(NoAgents),
            now_ms: system_now_ms,
            probe: Arc::new(NoProbe),
            worktree_root: default_worktree_root(),
            processes: Arc::new(SystemProcesses),
            export_dir: default_export_dir(),
            secrets: Arc::new(secrets::NoSecrets),
            forge: Arc::new(NoForge),
            forge_poll_every: None,
            claude_home: context::default_claude_home(),
            sandbox_evidence: policy::detect_sandbox,
            watcher: Arc::new(watcher::NoWatcher),
        }
    }
}

/// `~/Downloads`, where exported transcripts go.
pub fn default_export_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Downloads")
}

/// `~/.switchyard/worktrees`: short, ASCII, and outside every repository.
pub fn default_worktree_root() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join(".switchyard").join("worktrees")
}

pub use policy::detect_sandbox;

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
    forge: forge::Tracker,
    /// The core itself, for callbacks that re-enter it from other threads (the watcher).
    me: Weak<Mutex<Inner>>,
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
        let mut inner = Inner {
            data_dir: data_dir.to_path_buf(),
            events,
            store,
            runtime,
            sessions: Sessions::default(),
            shells: Shells::default(),
            compat,
            review: Default::default(),
            forge: Default::default(),
            me: Weak::new(),
        };
        // Reload sessions and recover from a quit or crash before anything else happens.
        sessions::load(&mut inner).map_err(|e| CoreError(format!("cannot read sessions from the store: {e}")))?;
        shells::kill_orphans(&inner);
        review::load(&mut inner);
        let every = inner.runtime.forge_poll_every;
        let inner = Arc::new(Mutex::new(inner));
        inner.lock().expect("core lock").me = Arc::downgrade(&inner);
        if let Some(every) = every {
            let weak = Arc::downgrade(&inner);
            std::thread::spawn(move || loop {
                std::thread::sleep(every);
                let Some(core) = weak.upgrade() else { break };
                forge::poll(&core);
            });
        }
        Ok(Core { inner })
    }

    /// One tick of the Forge poller: poll every worktree branch whose turn it is, and clean
    /// up those the Forge reports Merged. The core ticks itself every
    /// [`Runtime::forge_poll_every`] when that is set.
    pub fn poll_forge(&self) {
        forge::poll(&self.inner);
    }

    /// Run `program` with `args` in shell terminals instead of the user's login shell
    /// (`$SHELL -l`). Tests use a shell with no rc files.
    pub fn set_shell(&mut self, program: PathBuf, args: Vec<String>) {
        self.inner.lock().expect("core lock").shells.set_program(ShellProgram { program, args });
    }

    /// Handle one command. Commands that wait on the network (the Forge: `gh`, `git push`)
    /// hold the core only while they read or change its state, so agent output, shells and
    /// other commands carry on meanwhile; `dispatch` may be called from several threads.
    pub fn dispatch(&self, command: Command) {
        match command {
            Command::OpenPullRequest { worktree_path } => return forge::open_pull_request(&self.inner, &worktree_path),
            Command::PruneMerged { repository_id } => return forge::prune(&self.inner, &repository_id),
            Command::CheckForge => return forge::check(&self.inner),
            Command::PollForgeNow => return forge::poll_now(&self.inner),
            Command::WindowVisibility { visible } => return forge::set_visible(&self.inner, visible),
            Command::SyncTasks { repository_id } => return tasks::sync(&self.inner, &repository_id),
            Command::Attach => forge::detect_if_unknown(&self.inner),
            _ => {}
        }
        let mut inner = self.inner.lock().expect("core lock");
        match command {
            Command::Attach => inner.attach(),
            Command::AddRepository { path } => inner.add_repository(&path),
            Command::StartSession { worktree_path, agent, permission_mode, prompt } => {
                sessions::start(&mut inner, &self.inner, &worktree_path, agent, permission_mode, &prompt)
            }
            Command::StopSession { session_id } => sessions::stop(&mut inner, &session_id),
            Command::SendPrompt { session_id, text } => sessions::send_prompt(&mut inner, &self.inner, &session_id, &text),
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
                worktrees::create(&mut inner, command_name::CREATE_WORKTREE, &repository_id, &branch);
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
            Command::ResumeSession { session_id } => sessions::resume(&mut inner, &self.inner, &session_id),
            Command::ReplaceSession { session_id, agent, permission_mode, prompt } => {
                sessions::replace(&mut inner, &self.inner, &session_id, agent, permission_mode, &prompt)
            }
            Command::ReadArchivedSession { session_id } => sessions::read_archived(&inner, &session_id),
            Command::ExportTranscript { session_id } => sessions::export(&inner, &session_id),
            Command::UpdateSettings { settings } => sessions::update_settings(&mut inner, settings),
            Command::ReadChanges { worktree_path } => review::read(&mut inner, &worktree_path),
            Command::ResolveConflict { worktree_path, path, hunk_id, side } => {
                review::resolve_conflict(&mut inner, &worktree_path, &path, &hunk_id, side)
            }
            Command::DecideFile { worktree_path, path, decision } => {
                review::decide(&mut inner, &worktree_path, &path, decision)
            }
            Command::CommitAccepted { worktree_path, message } => review::commit(&mut inner, &worktree_path, &message),
            Command::DecideHunk { worktree_path, path, hunk_id, decision } => {
                review::decide_hunk(&mut inner, &worktree_path, &path, &hunk_id, decision)
            }
            Command::CommentHunk { worktree_path, path, hunk_id, comment } => {
                review::comment_hunk(&mut inner, &worktree_path, &path, &hunk_id, comment.as_deref())
            }
            Command::SendFeedback { worktree_path } => review::send_feedback(&mut inner, &self.inner, &worktree_path),
            Command::ReadPolicy { repository_id } => policy::read(&inner, &repository_id),
            Command::RevokeGrant { repository_id, grant_id } => policy::revoke(&inner, &repository_id, &grant_id),
            Command::RebaseWorktree { worktree_path } => rebase::rebase(&mut inner, &worktree_path),
            Command::AskAgentToRebase { session_id } => rebase::ask_agent(&mut inner, &self.inner, &session_id),
            Command::ReadHistory { worktree_path } => rebase::history(&inner, &worktree_path),
            Command::ReadContext { worktree_path } => context::read_context(&inner, &worktree_path),
            Command::AcknowledgePolicy { repository_id, hash } => policy::acknowledge(&inner, &repository_id, &hash),
            Command::ReadMcpServers { repository_id } => mcp::read(&inner, &repository_id),
            Command::EnableMcpServer { repository_id, name } => mcp::set_enabled(&inner, &repository_id, &name, true),
            Command::DisableMcpServer { repository_id, name } => mcp::set_enabled(&inner, &repository_id, &name, false),
            Command::ReadSecrets { repository_id } => secrets::read(&inner, &repository_id),
            Command::GrantSecret { repository_id, name } => secrets::set_grant(&mut inner, &repository_id, &name, true),
            Command::RevokeSecret { repository_id, name } => secrets::set_grant(&mut inner, &repository_id, &name, false),
            Command::AddSecret { repository_id, name } => secrets::add(&mut inner, &repository_id, &name),
            Command::RemoveSecret { repository_id, name } => secrets::remove(&mut inner, &repository_id, &name),
            Command::SetSecretValue { repository_id, name, value } => {
                secrets::set_value(&mut inner, &repository_id, &name, &value)
            }
            Command::CheckForge
            | Command::PollForgeNow
            | Command::WindowVisibility { .. }
            | Command::OpenPullRequest { .. }
            | Command::PruneMerged { .. }
            | Command::SyncTasks { .. } => {
                unreachable!("handled above without holding the core")
            }
            Command::LocalMerge { worktree_path } => forge::local_merge(&mut inner, &worktree_path),
            Command::ReadTasks { repository_id } => tasks::read(&inner, &repository_id),
            Command::CreateTask { repository_id, title, description, kind, checklist } => {
                tasks::create(&inner, &repository_id, tasks::NewTask { title, description, kind, checklist })
            }
            Command::TickTask { task_id, item, done } => tasks::tick(&inner, &task_id, item, done),
            Command::LinkTask { session_id, task_id, relink } => {
                tasks::link(&inner, &session_id, task_id.as_deref(), relink)
            }
            Command::StartTaskSession { task_id, target, agent, permission_mode, prompt } => {
                tasks::start_session(&mut inner, &self.inner, &task_id, target, agent, permission_mode, &prompt)
            }
        }
        sessions::flush(&mut inner);
    }
}

/// The refusal for a repository id the core does not know.
pub(crate) fn no_repository(id: &str) -> String {
    format!("There is no repository {id}")
}

/// The refusal for a session id the core does not know.
pub(crate) fn no_session(id: &str) -> String {
    format!("There is no session {id}")
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
        forge::replay_setup(self);
        let stored = match self.store.repositories() {
            Ok(list) => list,
            Err(e) => return self.refuse(command_name::ATTACH, format!("cannot read the store: {e}")),
        };
        for repo in stored {
            if let Err(reason) = self.announce(&repo) {
                self.refuse(command_name::ATTACH, reason);
            } else {
                self.watch_and_refresh(&repo);
            }
        }
        sessions::replay(self);
        shells::replay(self);
        forge::replay_pull_requests(self);
        tasks::replay_links(self);
    }

    fn add_repository(&mut self, path: &str) {
        const COMMAND: &str = command_name::ADD_REPOSITORY;
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
                } else {
                    self.watch_and_refresh(&repo);
                }
            }
            Err(e) => self.refuse(COMMAND, format!("cannot save the repository: {e}")),
        }
    }

    /// Watch each of an announced repository's worktrees and push their facts, so none
    /// waits for a change to have them.
    fn watch_and_refresh(&self, repo: &StoredRepository) {
        disk::watch_repository(self, Path::new(&repo.path));
        worktrees::refresh_repository(self, Path::new(&repo.path));
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
            remote: git::remote(dir),
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
        // Which repository `path` belongs to is read from its `.git` (no subprocess), so git
        // runs once, for that repository, instead of once per repository.
        if let Some(common) = git::common_dir(Path::new(path)) {
            let owner = repos.iter().find(|r| git::common_dir(Path::new(&r.path)).as_ref() == Some(&common));
            if let Some(repo) = owner {
                let wt = git::worktrees(Path::new(&repo.path)).ok()?.into_iter().find(|w| w.path == Path::new(path))?;
                return Some((repo.clone(), wt));
            }
        }
        repos.into_iter().find_map(|repo| {
            let list = git::worktrees(Path::new(&repo.path)).ok()?;
            let wt = list.into_iter().find(|w| w.path == Path::new(path))?;
            Some((repo, wt))
        })
    }
}
