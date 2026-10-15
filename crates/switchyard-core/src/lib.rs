//! Switchyard core. Contains no Tauri types (ADR-0001).
//!
//! The only way in is [`Core::dispatch`] with a schema [`Command`]; the only way out is the
//! [`Event`] channel handed to [`Core::open`]. The Tauri app, tests and any future
//! standalone binary all drive the core through that pair.

mod git;
mod store;

use std::path::Path;
use std::sync::mpsc::Sender;

use switchyard_schema::{Command, Event, Repository, Worktree};

use store::{Store, StoredRepository};

pub struct Core {
    events: Sender<Event>,
    store: Store,
}

#[derive(Debug)]
pub struct CoreError(pub String);

impl Core {
    /// Open the core on `data_dir` (created if missing), emitting events into `events`.
    pub fn open(data_dir: &Path, events: Sender<Event>) -> Result<Core, CoreError> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| CoreError(format!("cannot create {}: {e}", data_dir.display())))?;
        let store = Store::open(data_dir).map_err(|e| CoreError(format!("cannot open store: {e}")))?;
        Ok(Core { events, store })
    }

    pub fn dispatch(&mut self, command: Command) {
        match command {
            Command::Attach => self.attach(),
            Command::AddRepository { path } => self.add_repository(&path),
        }
    }

    fn emit(&self, event: Event) {
        // A closed channel means nobody is listening; the core carries on regardless.
        let _ = self.events.send(event);
    }

    fn refuse(&self, command: &str, reason: String) {
        self.emit(Event::CommandRefused { command: command.into(), reason });
    }

    fn attach(&mut self) {
        self.emit(Event::Attached);
        let stored = match self.store.repositories() {
            Ok(list) => list,
            Err(e) => return self.refuse("attach", format!("cannot read the store: {e}")),
        };
        for repo in stored {
            if let Err(reason) = self.announce(&repo) {
                self.refuse("attach", reason);
            }
        }
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
}
