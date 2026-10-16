//! Configuration edited on disk reaches the UI without a click (ADR-0008): the core watches
//! the instruction files, Claude's project memory, the policy, `.mcp.json` and the local
//! tasks file, and on a change pushes the same events the in-app commands do. The watcher is
//! fake here: `disk_changed` delivers a notification by hand.
mod harness;

use std::path::{Path, PathBuf};

use harness::{added_repository, Harness, TempRepo};
use switchyard_schema::{Command, ContextFile, DeclaredMcpServer, Event, Policy, TaskKind};

fn path(p: &Path) -> String {
    p.to_str().unwrap().into()
}

/// A repository with one linked worktree on `feat/config`, added and attached.
struct Setup {
    repo: TempRepo,
    worktree: PathBuf,
    repo_id: String,
    core: Harness,
}

fn attached() -> Setup {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/config");
    let mut core = Harness::launch();
    let repo_id = added_repository(&core.add_repository(&repo.path)).id;
    core.send(Command::Attach);
    Setup { repo, worktree, repo_id, core }
}

/// The context pushed for `worktree` in `events` (the last one).
fn context_of(events: &[Event], worktree: &Path) -> Vec<ContextFile> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::ContextRead { worktree_path, files } if Path::new(worktree_path) == worktree => Some(files.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected contextRead for {worktree:?} in {events:#?}"))
}

fn texts(files: &[ContextFile]) -> Vec<(&str, &str)> {
    files.iter().map(|f| (f.name.as_str(), f.text.as_str())).collect()
}

#[test]
fn an_instruction_file_edited_on_disk_pushes_the_worktrees_context() {
    let mut s = attached();

    std::fs::write(s.worktree.join("CLAUDE.md"), "Use pnpm.\n").unwrap();
    let events = s.core.disk_changed(&s.worktree, &[&s.worktree.join("CLAUDE.md")]);

    assert_eq!(texts(&context_of(&events, &s.worktree)), vec![("CLAUDE.md", "Use pnpm.\n")]);
}

#[test]
fn an_ordinary_file_edit_rereads_no_configuration() {
    let mut s = attached();

    std::fs::write(s.worktree.join("README"), "edited\n").unwrap();
    let events = s.core.disk_changed(&s.worktree, &[&s.worktree.join("README")]);

    let config: Vec<&Event> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::ContextRead { .. }
                    | Event::PolicyChanged { .. }
                    | Event::McpServersChanged { .. }
                    | Event::TasksChanged { .. }
            )
        })
        .collect();
    assert!(config.is_empty(), "{config:#?}");
}

/// Claude Code keeps a project's memory under `projects/<path with every non-alphanumeric
/// character as '-'>/memory`.
fn memory_dir(core: &Harness, worktree: &Path) -> PathBuf {
    let slug: String = path(worktree).chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    core.claude_home().join("projects").join(slug).join("memory")
}

#[test]
fn the_agent_writing_its_memory_pushes_the_worktrees_context() {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("feat/config");
    let mut core = Harness::launch();
    let memory = memory_dir(&core, &worktree);
    std::fs::create_dir_all(&memory).unwrap();
    added_repository(&core.add_repository(&repo.path));
    core.send(Command::Attach);
    assert!(
        core.watcher.roots(&worktree).iter().any(|root| memory.starts_with(root)),
        "memory is watched: {:?}",
        core.watcher.roots(&worktree)
    );

    std::fs::write(memory.join("MEMORY.md"), "- [Prefers pnpm](pnpm.md)\n").unwrap();
    let events = core.disk_changed(&worktree, &[&memory.join("MEMORY.md")]);

    assert_eq!(texts(&context_of(&events, &worktree)), vec![("MEMORY.md", "- [Prefers pnpm](pnpm.md)\n")]);
}

#[test]
fn memory_claude_makes_during_a_turn_is_read_at_its_end_and_watched_from_then_on() {
    let mut s = attached();
    let memory = memory_dir(&s.core, &s.worktree);
    harness::started_session(&s.core.start_session(&s.worktree, "Remember pnpm."));
    assert!(!s.core.watcher.roots(&s.worktree).iter().any(|root| memory.starts_with(root)));

    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join("MEMORY.md"), "- pnpm\n").unwrap();
    let events = s.core.agent_says(0, adapter_protocol::Event::TurnComplete);

    assert_eq!(texts(&context_of(&events, &s.worktree)), vec![("MEMORY.md", "- pnpm\n")]);
    assert!(
        s.core.watcher.roots(&s.worktree).iter().any(|root| memory.starts_with(root)),
        "memory is watched: {:?}",
        s.core.watcher.roots(&s.worktree)
    );
}

/// The policy pushed in `events` (the last one).
fn policy_in(events: &[Event]) -> Policy {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::PolicyChanged { policy, .. } => Some(policy.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected policyChanged in {events:#?}"))
}

fn write_policy(repo: &TempRepo, policy: &str) {
    std::fs::create_dir_all(repo.path.join(".switchyard")).unwrap();
    std::fs::write(repo.path.join(".switchyard/policy.yml"), policy).unwrap();
}

#[test]
fn an_allow_added_to_the_policy_on_disk_shows_as_a_request_and_answers_nothing() {
    let mut s = attached();
    let policy_file = s.repo.path.join(".switchyard/policy.yml");

    write_policy(&s.repo, "permissions:\n  allow:\n    - command: pnpm *\n");
    let policy = policy_in(&s.core.disk_changed(&s.repo.path, &[&policy_file]));

    let request = policy.request.expect("the new allow is a request");
    assert_eq!(request.allow.len(), 1, "{request:?}");
    assert!(policy.grants.iter().all(|g| g.id != "repo-allow-1"), "not applied: {:?}", policy.grants);
}

#[test]
fn an_acknowledged_policy_expanded_on_disk_asks_again() {
    let mut s = attached();
    let policy_file = s.repo.path.join(".switchyard/policy.yml");
    write_policy(&s.repo, "permissions:\n  allow:\n    - command: pnpm *\n");
    let hash = policy_in(&s.core.disk_changed(&s.repo.path, &[&policy_file])).request.expect("a request").hash;
    s.core.send(Command::AcknowledgePolicy { repository_id: s.repo_id.clone(), hash });

    write_policy(&s.repo, "permissions:\n  allow:\n    - command: pnpm *\n    - command: cargo *\n");
    let policy = policy_in(&s.core.disk_changed(&s.repo.path, &[&policy_file]));

    assert!(policy.request.is_some(), "the expanded allow asks again: {policy:?}");
}

/// The declared MCP servers pushed in `events` (the last list).
fn declared_in(events: &[Event]) -> Vec<DeclaredMcpServer> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::McpServersChanged { servers, .. } => Some(servers.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected mcpServersChanged in {events:#?}"))
}

fn docs_server(url: &str) -> String {
    format!(r#"{{ "mcpServers": {{ "docs": {{ "type": "http", "url": "{url}" }} }} }}"#)
}

#[test]
fn an_mcp_server_edited_on_disk_is_declared_not_enabled() {
    let mut s = attached();
    let mcp_file = s.repo.path.join(".mcp.json");
    std::fs::write(&mcp_file, docs_server("https://docs.example.com/mcp")).unwrap();
    s.core.disk_changed(&s.repo.path, &[&mcp_file]);
    let enabled = declared_in(&s.core.send(Command::EnableMcpServer { repository_id: s.repo_id.clone(), name: "docs".into() }));
    assert!(enabled[0].enabled, "{enabled:?}");

    std::fs::write(&mcp_file, docs_server("https://elsewhere.example.com/mcp")).unwrap();
    let servers = declared_in(&s.core.disk_changed(&s.repo.path, &[&mcp_file]));

    let summary: Vec<(&str, bool, bool)> = servers.iter().map(|s| (s.name.as_str(), s.enabled, s.changed)).collect();
    assert_eq!(summary, vec![("docs", false, true)]);
}

#[test]
fn the_local_tasks_file_edited_by_hand_pushes_the_tasks() {
    let mut s = attached();
    s.core.send(Command::CreateTask {
        repository_id: s.repo_id.clone(),
        title: "Add login".into(),
        description: String::new(),
        kind: TaskKind::Feat,
        checklist: vec![],
    });
    let tasks_dir = s.core.data_dir().join("tasks");
    let file = std::fs::read_dir(&tasks_dir).unwrap().next().expect("the local tasks file").unwrap().path();
    assert!(
        s.core.watcher.roots(&s.repo.path).iter().any(|root| file.starts_with(root)),
        "the tasks file is watched: {:?}",
        s.core.watcher.roots(&s.repo.path)
    );

    let text = std::fs::read_to_string(&file).unwrap().replace("Add login", "Add login with SSO");
    std::fs::write(&file, text).unwrap();
    let events = s.core.disk_changed(&s.repo.path, &[&file]);

    let titles: Vec<String> = events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::TasksChanged { repository_id, tasks, .. } if *repository_id == s.repo_id => {
                Some(tasks.iter().map(|t| t.title.clone()).collect())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected tasksChanged in {events:#?}"));
    assert_eq!(titles, vec!["Add login with SSO".to_string()]);
}
