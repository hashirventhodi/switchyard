//! Switchyard core. Contains no Tauri types (ADR-0001).
//!
//! The only way in is [`Core::dispatch`] with a schema [`Command`]; the only way out is the
//! [`Event`] channel handed to [`Core::open`]. The Tauri app, tests and any future
//! standalone binary all drive the core through that pair.

mod git;

use std::path::Path;
use std::sync::mpsc::Sender;

use switchyard_schema::{Command, Event, Repository, Worktree};

pub struct Core {
    events: Sender<Event>,
    next_id: u64,
}

#[derive(Debug)]
pub struct CoreError(pub String);

impl Core {
    /// Open the core on `data_dir`, emitting events into `events`.
    pub fn open(_data_dir: &Path, events: Sender<Event>) -> Result<Core, CoreError> {
        Ok(Core { events, next_id: 1 })
    }

    pub fn dispatch(&mut self, command: Command) {
        match command {
            Command::Attach => {}
            Command::AddRepository { path } => self.add_repository(&path),
        }
    }

    fn emit(&self, event: Event) {
        // A closed channel means nobody is listening; the core carries on regardless.
        let _ = self.events.send(event);
    }

    fn add_repository(&mut self, path: &str) {
        let worktrees = git::worktrees(Path::new(path)).unwrap();
        let primary = &worktrees[0];
        let repository = Repository {
            id: format!("repo-{}", self.next_id),
            name: primary.path.file_name().unwrap().to_string_lossy().into_owned(),
            path: primary.path.to_string_lossy().into_owned(),
            base_branch: git::base_branch(&primary.path, primary),
        };
        self.next_id += 1;
        let id = repository.id.clone();
        self.emit(Event::RepositoryAdded { repository });
        for wt in worktrees {
            self.emit(Event::WorktreeAdded {
                repository_id: id.clone(),
                worktree: Worktree {
                    path: wt.path.to_string_lossy().into_owned(),
                    branch: wt.branch,
                    primary: wt.primary,
                },
            });
        }
    }
}
