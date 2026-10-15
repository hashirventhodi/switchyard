//! Context: the files an agent reads as instructions in a worktree (CLAUDE.md, AGENTS.md
//! and the like) and Claude Code's memory for it, read-only, through the core.

mod harness;

use std::path::Path;

use harness::{worktree_snapshot, Harness, TempRepo};
use switchyard_schema::{Command, ContextFile, ContextKind, Event};

fn path(p: &Path) -> String {
    p.to_str().unwrap().into()
}

fn context_read(events: &[Event]) -> Vec<ContextFile> {
    events
        .iter()
        .find_map(|e| match e {
            Event::ContextRead { files, .. } => Some(files.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected contextRead in {events:#?}"))
}

/// Claude Code keeps a project's memory under `projects/<path with every non-alphanumeric
/// character as '-'>/memory`.
fn memory_dir(core: &Harness, worktree: &Path) -> std::path::PathBuf {
    let slug: String = path(worktree).chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    core.claude_home().join("projects").join(slug).join("memory")
}

#[test]
fn context_lists_instruction_files_and_memory_without_writing_anything() {
    let repo = TempRepo::new("acme");
    let wt = repo.add_worktree("feat/x");
    std::fs::write(wt.join("CLAUDE.md"), "# acme\nUse pnpm.\n").unwrap();
    std::fs::write(wt.join("AGENTS.md"), "Run the tests first.\n").unwrap();
    std::fs::create_dir_all(wt.join(".claude")).unwrap();
    std::fs::write(wt.join(".claude").join("CLAUDE.md"), "Nested rules.\n").unwrap();
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    let memory = memory_dir(&core, &wt);
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join("MEMORY.md"), "- [Prefers pnpm](pnpm.md)\n").unwrap();
    std::fs::write(memory.join("pnpm.md"), "The user prefers pnpm.\n").unwrap();
    let before = worktree_snapshot(&wt);

    let files = context_read(&core.send(Command::ReadContext { worktree_path: path(&wt) }));

    let summary: Vec<(&str, ContextKind, &str)> =
        files.iter().map(|f| (f.name.as_str(), f.kind, f.text.as_str())).collect();
    assert_eq!(
        summary,
        vec![
            ("CLAUDE.md", ContextKind::Instructions, "# acme\nUse pnpm.\n"),
            (".claude/CLAUDE.md", ContextKind::Instructions, "Nested rules.\n"),
            ("AGENTS.md", ContextKind::Instructions, "Run the tests first.\n"),
            ("MEMORY.md", ContextKind::Memory, "- [Prefers pnpm](pnpm.md)\n"),
            ("pnpm.md", ContextKind::Memory, "The user prefers pnpm.\n"),
        ]
    );
    assert_eq!(files[3].path, path(&memory.join("MEMORY.md")));
    assert!(files.iter().all(|f| !f.truncated));
    assert_eq!(worktree_snapshot(&wt), before, "reading context writes nothing");
}

#[test]
fn a_huge_context_file_is_cut_short_and_said_so() {
    let repo = TempRepo::new("acme");
    let wt = repo.add_worktree("feat/x");
    std::fs::write(wt.join("CLAUDE.md"), "x".repeat(200 * 1024)).unwrap();
    let mut core = Harness::launch();
    core.add_repository(&repo.path);
    let files = context_read(&core.send(Command::ReadContext { worktree_path: path(&wt) }));
    assert!(files[0].truncated);
    assert_eq!(files[0].text.len(), 64 * 1024);
}

#[test]
fn context_of_an_unknown_worktree_is_refused() {
    let mut core = Harness::launch();
    let events = core.send(Command::ReadContext { worktree_path: "/nowhere".into() });
    assert!(matches!(&events[..], [Event::CommandRefused { command, .. }] if command == "readContext"), "{events:#?}");
}
