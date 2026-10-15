//! Local tasks: a JSON file per Repository in Switchyard's data directory (never in the
//! repository itself, so adding a task never dirties a worktree). Keys are `L-1`, `L-2`…
//! and are never reused.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use switchyard_schema::{TaskColumn, TaskKind, TaskSourceKind};

use super::{NewTask, SourceTask, TaskSource};

pub(crate) struct LocalTasks {
    file: PathBuf,
}

#[derive(Default, Serialize, Deserialize)]
struct TaskFile {
    /// The number the next task's key takes.
    next: u32,
    tasks: Vec<LocalTask>,
}

#[derive(Serialize, Deserialize)]
struct LocalTask {
    key: String,
    title: String,
    description: String,
    kind: TaskKind,
    column: TaskColumn,
    checklist: Vec<String>,
}

impl LocalTasks {
    /// The local tasks of the repository whose folder (`worktrees::repository_folder`) is
    /// `folder`, under the data directory.
    pub(crate) fn new(data_dir: &Path, folder: &str) -> Self {
        LocalTasks { file: data_dir.join("tasks").join(format!("{folder}.json")) }
    }

    fn read(&self) -> Result<TaskFile, String> {
        match std::fs::read_to_string(&self.file) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("the local task file {} is not valid: {e}", self.file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TaskFile { next: 1, tasks: Vec::new() }),
            Err(e) => Err(format!("cannot read the local task file {}: {e}", self.file.display())),
        }
    }

    /// Written whole to a temporary file, then renamed over the old one.
    fn write(&self, file: &TaskFile) -> Result<(), String> {
        let fail = |e: std::io::Error| format!("cannot write the local task file {}: {e}", self.file.display());
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let text = serde_json::to_string_pretty(file).expect("task files serialize");
        let temp = self.file.with_extension("json.tmp");
        std::fs::write(&temp, text).map_err(fail)?;
        std::fs::rename(&temp, &self.file).map_err(fail)
    }
}

fn source_task(t: &LocalTask) -> SourceTask {
    SourceTask {
        key: t.key.clone(),
        title: t.title.clone(),
        description: t.description.clone(),
        kind: t.kind,
        column: t.column,
        checklist: t.checklist.clone(),
        url: None,
    }
}

impl TaskSource for LocalTasks {
    fn kind(&self) -> TaskSourceKind {
        TaskSourceKind::Local
    }

    fn tasks(&self) -> Result<Vec<SourceTask>, String> {
        Ok(self.read()?.tasks.iter().map(source_task).collect())
    }

    fn move_to(&mut self, key: &str, column: TaskColumn, _pull_request: Option<&str>) -> Result<(), String> {
        let mut file = self.read()?;
        let Some(task) = file.tasks.iter_mut().find(|t| t.key == key) else {
            return Err(format!("there is no local task {key}"));
        };
        task.column = column;
        self.write(&file)
    }

    fn create(&mut self, new: NewTask) -> Result<SourceTask, String> {
        let mut file = self.read()?;
        let n = file.next.max(1);
        file.next = n + 1;
        let task = LocalTask {
            key: format!("L-{n}"),
            title: new.title,
            description: new.description,
            kind: new.kind,
            column: TaskColumn::Todo,
            checklist: new.checklist,
        };
        let created = source_task(&task);
        file.tasks.push(task);
        self.write(&file)?;
        Ok(created)
    }
}
