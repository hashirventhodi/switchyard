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

#[test]
fn spend_records_keep_reported_and_estimated_cost_apart() {
    use switchyard_schema::Spend;
    let event = Event::SpendRecorded {
        session_id: "session-1".into(),
        at_ms: 1_760_000_000_000,
        spend: Spend {
            input_tokens: 300_000,
            output_tokens: 50_000,
            cache_read_tokens: 1_000_000,
            reported_usd: None,
            estimated_usd: Some(1.6),
            price_table_version: Some("2026-10-15".into()),
        },
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({
            "type": "spendRecorded",
            "sessionId": "session-1",
            "atMs": 1_760_000_000_000u64,
            "spend": {
                "inputTokens": 300_000,
                "outputTokens": 50_000,
                "cacheReadTokens": 1_000_000,
                "reportedUsd": null,
                "estimatedUsd": 1.6,
                "priceTableVersion": "2026-10-15"
            }
        })
    );
}

#[test]
fn worktree_lifecycle_commands_and_events_use_camel_case() {
    use switchyard_schema::WorktreeFacts;
    let command: Command =
        serde_json::from_value(json!({"type": "createWorktree", "repositoryId": "repo-1", "branch": "feat/x"}))
            .unwrap();
    assert_eq!(command, Command::CreateWorktree { repository_id: "repo-1".into(), branch: "feat/x".into() });
    let command: Command =
        serde_json::from_value(json!({"type": "removeWorktree", "worktreePath": "/wt/x"})).unwrap();
    assert_eq!(command, Command::RemoveWorktree { worktree_path: "/wt/x".into() });
    let refresh: Command = serde_json::from_value(json!({"type": "refreshWorktrees"})).unwrap();
    assert_eq!(refresh, Command::RefreshWorktrees);

    let event = Event::WorktreeFactsChanged {
        worktree_path: "/wt/x".into(),
        facts: WorktreeFacts { ahead: 1, behind: 2, dirty: 3, conflicts: vec!["src/a.rs".into()] },
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type": "worktreeFactsChanged", "worktreePath": "/wt/x", "facts": {"ahead": 1, "behind": 2, "dirty": 3, "conflicts": ["src/a.rs"]}})
    );
}

#[test]
fn rebase_and_history_commands_and_events_are_camel_case() {
    use switchyard_schema::Commit;
    let rebase: Command = serde_json::from_value(json!({"type": "rebaseWorktree", "worktreePath": "/wt/x"})).unwrap();
    assert_eq!(rebase, Command::RebaseWorktree { worktree_path: "/wt/x".into() });
    let ask: Command = serde_json::from_value(json!({"type": "askAgentToRebase", "sessionId": "session-1"})).unwrap();
    assert_eq!(ask, Command::AskAgentToRebase { session_id: "session-1".into() });
    let history: Command = serde_json::from_value(json!({"type": "readHistory", "worktreePath": "/wt/x"})).unwrap();
    assert_eq!(history, Command::ReadHistory { worktree_path: "/wt/x".into() });

    let rebased = Event::WorktreeRebased { worktree_path: "/wt/x".into(), base: "main".into() };
    assert_eq!(
        serde_json::to_value(rebased).unwrap(),
        json!({"type": "worktreeRebased", "worktreePath": "/wt/x", "base": "main"})
    );
    let event = Event::WorktreeHistory {
        worktree_path: "/wt/x".into(),
        base: "main".into(),
        commits: vec![Commit {
            hash: "abc123".into(),
            short: "abc1".into(),
            subject: "Add login".into(),
            author: "Ada".into(),
            at_ms: 1_000,
        }],
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type": "worktreeHistory", "worktreePath": "/wt/x", "base": "main", "commits": [
            {"hash": "abc123", "short": "abc1", "subject": "Add login", "author": "Ada", "atMs": 1000}
        ]})
    );
}

#[test]
fn persistence_commands_and_events_use_camel_case() {
    use switchyard_schema::{AgentKind, ArchivedSession, PermissionMode, Session, SessionStatus, Settings};
    let replace: Command = serde_json::from_value(json!({
        "type": "replaceSession", "sessionId": "session-1", "agent": "claudeCode",
        "permissionMode": "autoEdit", "prompt": "again"
    }))
    .unwrap();
    assert_eq!(
        replace,
        Command::ReplaceSession {
            session_id: "session-1".into(),
            agent: AgentKind::ClaudeCode,
            permission_mode: PermissionMode::AutoEdit,
            prompt: "again".into()
        }
    );
    let update: Command = serde_json::from_value(json!({
        "type": "updateSettings", "settings": {"archiveKeepPerRepository": 10, "archiveKeepDays": 7}
    }))
    .unwrap();
    assert_eq!(update, Command::UpdateSettings { settings: Settings { archive_keep_per_repository: 10, archive_keep_days: 7 } });
    let resume: Command = serde_json::from_value(json!({"type": "resumeSession", "sessionId": "session-1"})).unwrap();
    assert_eq!(resume, Command::ResumeSession { session_id: "session-1".into() });

    let session = Session {
        id: "session-1".into(),
        repository_id: "repo-1".into(),
        worktree_path: "/wt".into(),
        branch: Some("fix/a".into()),
        agent: AgentKind::ClaudeCode,
        permission_mode: PermissionMode::AskFirst,
        status: SessionStatus::Idle,
        started_at_ms: 5,
    };
    let event = Event::SessionArchived { archived: ArchivedSession { session, archived_at_ms: 9, entries: 3 } };
    let value = serde_json::to_value(event).unwrap();
    assert_eq!(value["type"], "sessionArchived");
    assert_eq!(value["archived"]["archivedAtMs"], 9);
    assert_eq!(value["archived"]["session"]["id"], "session-1");
    let pruned = Event::ArchivedSessionPruned { repository_id: "repo-1".into(), session_id: "session-1".into() };
    assert_eq!(
        serde_json::to_value(pruned).unwrap(),
        json!({"type": "archivedSessionPruned", "repositoryId": "repo-1", "sessionId": "session-1"})
    );
}

#[test]
fn review_commands_and_events_use_camel_case() {
    use switchyard_schema::{ChangedFile, Decision, DiffLine, FileStatus, Hunk, LineKind};
    let command: Command = serde_json::from_value(
        json!({"type": "decideFile", "worktreePath": "/wt/x", "path": "a.rs", "decision": "accept"}),
    )
    .unwrap();
    assert_eq!(
        command,
        Command::DecideFile { worktree_path: "/wt/x".into(), path: "a.rs".into(), decision: Some(Decision::Accept) }
    );
    let command: Command =
        serde_json::from_value(json!({"type": "commitAccepted", "worktreePath": "/wt/x", "message": "m"})).unwrap();
    assert_eq!(command, Command::CommitAccepted { worktree_path: "/wt/x".into(), message: "m".into() });

    let event = Event::ChangesRead {
        worktree_path: "/wt/x".into(),
        files: vec![ChangedFile {
            path: "b.rs".into(),
            old_path: Some("a.rs".into()),
            status: FileStatus::Renamed,
            added: 1,
            removed: 0,
            binary: false,
            decision: None,
            hunks: vec![Hunk {
                id: "00ff".into(),
                header: "@@ -1 +1,2 @@".into(),
                lines: vec![DiffLine { kind: LineKind::Added, old: None, new: Some(2), text: "x".into() }],
            }],
        }],
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type": "changesRead", "worktreePath": "/wt/x", "files": [{
            "path": "b.rs", "oldPath": "a.rs", "status": "renamed", "added": 1, "removed": 0,
            "binary": false, "decision": null,
            "hunks": [{"id": "00ff", "header": "@@ -1 +1,2 @@",
                       "lines": [{"kind": "added", "old": null, "new": 2, "text": "x"}]}]
        }]})
    );
}
