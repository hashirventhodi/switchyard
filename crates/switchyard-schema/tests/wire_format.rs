//! The renderer depends on this exact JSON shape; it is the contract the generated
//! TypeScript describes.

use serde_json::json;
use switchyard_schema::{Command, Event, Worktree};

#[test]
fn commands_are_tagged_by_type_with_camel_case_names() {
    let command: Command =
        serde_json::from_value(json!({"type": "addRepository", "path": "/r"})).unwrap();
    assert_eq!(command, Command::AddRepository { path: "/r".into() });

    let attach: Command = serde_json::from_value(json!({"type": "attach"})).unwrap();
    assert_eq!(attach, Command::Attach);
}

#[test]
fn events_serialize_with_camel_case_fields() {
    let event = Event::WorktreeAdded {
        repository_id: "repo-1".into(),
        worktree: Worktree { path: "/r".into(), branch: Some("main".into()), primary: true },
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({
            "type": "worktreeAdded",
            "repositoryId": "repo-1",
            "worktree": {"path": "/r", "branch": "main", "primary": true}
        })
    );
}

#[test]
fn session_commands_and_transcript_entries_use_camel_case() {
    use switchyard_schema::{AgentKind, Entry, PermissionMode, ToolState};
    let command: Command = serde_json::from_value(json!({
        "type": "startSession",
        "worktreePath": "/wt/a",
        "agent": "claudeCode",
        "permissionMode": "askFirst",
        "prompt": "Fix it"
    }))
    .unwrap();
    assert_eq!(
        command,
        Command::StartSession {
            worktree_path: "/wt/a".into(),
            agent: AgentKind::ClaudeCode,
            permission_mode: PermissionMode::AskFirst,
            prompt: "Fix it".into(),
        }
    );
    let event = Event::TranscriptEntry {
        session_id: "session-1".into(),
        index: 3,
        entry: Entry::Tool {
            call_id: "t1".into(),
            tool: "Bash".into(),
            summary: "ls".into(),
            state: ToolState::Ok,
            output: Some("a".into()),
        },
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({
            "type": "transcriptEntry",
            "sessionId": "session-1",
            "index": 3,
            "entry": {"type": "tool", "callId": "t1", "tool": "Bash", "summary": "ls", "state": "ok", "output": "a"}
        })
    );
}
