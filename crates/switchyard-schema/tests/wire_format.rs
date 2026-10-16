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
    use switchyard_schema::{AgentKind, Entry, ModelChoice, PermissionMode, ToolState};
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
            // Sent without a Model (as before #34): Default.
            model: ModelChoice::Default,
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
fn spend_records_keep_reported_and_estimated_cost_apart_per_model() {
    use switchyard_schema::{ModelSpend, Spend};
    let model = |name: &str, input_tokens, reported_usd, estimated_usd: Option<f64>| ModelSpend {
        model: Some(name.into()),
        input_tokens,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reported_usd,
        estimated_usd,
        price_table_version: estimated_usd.map(|_| "2026-10-15".into()),
    };
    let event = Event::SpendRecorded {
        session_id: "session-1".into(),
        at_ms: 1_760_000_000_000,
        spend: Spend {
            input_tokens: 300_000,
            output_tokens: 50_000,
            cache_read_tokens: 1_000_000,
            cache_write_tokens: 200_000,
            reported_usd: Some(0.25),
            estimated_usd: Some(1.6),
            price_table_version: Some("2026-10-15".into()),
            by_model: vec![
                model("gpt-5.6-terra", 100_000, None, Some(1.6)),
                model("claude-haiku-4-5", 150_000, Some(0.25), None),
                model("mystery", 50_000, None, None),
            ],
        },
    };
    let row = |name: &str, input: u64, reported: Option<f64>, estimated: Option<f64>| {
        json!({
            "model": name,
            "inputTokens": input,
            "outputTokens": 0,
            "cacheReadTokens": 0,
            "cacheWriteTokens": 0,
            "reportedUsd": reported,
            "estimatedUsd": estimated,
            "priceTableVersion": estimated.map(|_| "2026-10-15")
        })
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
                "cacheWriteTokens": 200_000,
                "reportedUsd": 0.25,
                "estimatedUsd": 1.6,
                "priceTableVersion": "2026-10-15",
                "byModel": [
                    row("gpt-5.6-terra", 100_000, None, Some(1.6)),
                    row("claude-haiku-4-5", 150_000, Some(0.25), None),
                    row("mystery", 50_000, None, None)
                ]
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
        facts: WorktreeFacts { ahead: 1, behind: 2, dirty: 3, conflicts: vec!["src/a.rs".into()], changed_files: 3, commits: 1 },
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type": "worktreeFactsChanged", "worktreePath": "/wt/x", "facts": {"ahead": 1, "behind": 2, "dirty": 3, "conflicts": ["src/a.rs"], "changedFiles": 3, "commits": 1}})
    );

    let changed = Event::WorktreeChanged {
        worktree_path: "/wt/x".into(),
        kinds: vec![switchyard_schema::ChangeKind::Files, switchyard_schema::ChangeKind::Git],
    };
    assert_eq!(
        serde_json::to_value(changed).unwrap(),
        json!({"type": "worktreeChanged", "worktreePath": "/wt/x", "kinds": ["files", "git"]})
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
fn a_session_starts_on_a_chosen_model_or_on_default() {
    use switchyard_schema::{AgentKind, ModelChoice, PermissionMode};
    let chosen: Command = serde_json::from_value(json!({
        "type": "startSession", "worktreePath": "/wt/a", "agent": "claudeCode", "permissionMode": "explore",
        "model": {"type": "model", "id": "claude-sonnet-5-5"}, "prompt": "Look around"
    }))
    .unwrap();
    assert_eq!(
        chosen,
        Command::StartSession {
            worktree_path: "/wt/a".into(),
            agent: AgentKind::ClaudeCode,
            permission_mode: PermissionMode::Explore,
            model: ModelChoice::Model { id: "claude-sonnet-5-5".into() },
            prompt: "Look around".into(),
        }
    );
    assert_eq!(serde_json::to_value(ModelChoice::Default).unwrap(), json!({"type": "default"}));
    assert_eq!(ModelChoice::Model { id: "claude-sonnet-5-5".into() }.id(), Some("claude-sonnet-5-5"));
    assert_eq!(ModelChoice::Default.id(), None);
}

#[test]
fn persistence_commands_and_events_use_camel_case() {
    use switchyard_schema::{AgentKind, ArchivedSession, ModelChoice, PermissionMode, Session, SessionStatus, Settings};
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
            model: ModelChoice::Default,
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
        model: ModelChoice::Default,
        switched_model: None,
        pending_model: None,
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
    use switchyard_schema::{ChangedFile, Decision, DiffLine, FileStatus, Hunk, LineKind, StaleHunk};
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
            decision: Some(Decision::Accept),
            stale: true,
            hunks: vec![Hunk {
                id: "00ff".into(),
                header: "@@ -1 +1,2 @@".into(),
                lines: vec![DiffLine { kind: LineKind::Added, old: None, new: Some(2), text: "x".into() }],
                decision: Some(Decision::Reject),
                comment: Some("why?".into()),
                sent: false,
            }],
            stale_hunks: vec![StaleHunk {
                id: "11aa".into(),
                header: "@@ -5 +5 @@".into(),
                decision: Decision::Reject,
                comment: None,
                sent: true,
            }],
        }],
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type": "changesRead", "worktreePath": "/wt/x", "files": [{
            "path": "b.rs", "oldPath": "a.rs", "status": "renamed", "added": 1, "removed": 0,
            "binary": false, "decision": "accept", "stale": true,
            "hunks": [{"id": "00ff", "header": "@@ -1 +1,2 @@",
                       "lines": [{"kind": "added", "old": null, "new": 2, "text": "x"}],
                       "decision": "reject", "comment": "why?", "sent": false}],
            "staleHunks": [{"id": "11aa", "header": "@@ -5 +5 @@", "decision": "reject", "comment": null, "sent": true}]
        }]})
    );
    let event = Event::Committed {
        worktree_path: "/wt/x".into(),
        branch: Some("feat/x".into()),
        commit: "abc1234".into(),
        files: vec!["a.rs".into()],
        skipped_stale: vec!["b.rs".into()],
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type": "committed", "worktreePath": "/wt/x", "branch": "feat/x", "commit": "abc1234",
               "files": ["a.rs"], "skippedStale": ["b.rs"]})
    );
}

#[test]
fn hunk_feedback_wire_format() {
    use switchyard_schema::Decision;
    let cmd = Command::DecideHunk {
        worktree_path: "/wt/x".into(),
        path: "a.rs".into(),
        hunk_id: "00ff".into(),
        decision: Some(Decision::Reject),
    };
    assert_eq!(
        serde_json::to_value(cmd).unwrap(),
        json!({"type": "decideHunk", "worktreePath": "/wt/x", "path": "a.rs", "hunkId": "00ff", "decision": "reject"})
    );
    let cmd = Command::CommentHunk { worktree_path: "/wt/x".into(), path: "a.rs".into(), hunk_id: "00ff".into(), comment: None };
    assert_eq!(
        serde_json::to_value(cmd).unwrap(),
        json!({"type": "commentHunk", "worktreePath": "/wt/x", "path": "a.rs", "hunkId": "00ff", "comment": null})
    );
    assert_eq!(
        serde_json::to_value(Command::SendFeedback { worktree_path: "/wt/x".into() }).unwrap(),
        json!({"type": "sendFeedback", "worktreePath": "/wt/x"})
    );
    let event = Event::HunkReviewed {
        worktree_path: "/wt/x".into(),
        path: "a.rs".into(),
        hunk_id: "00ff".into(),
        decision: None,
        comment: Some("c".into()),
        sent: true,
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type": "hunkReviewed", "worktreePath": "/wt/x", "path": "a.rs", "hunkId": "00ff",
               "decision": null, "comment": "c", "sent": true})
    );
    let event = Event::FeedbackSent { worktree_path: "/wt/x".into(), session_id: "session-1".into(), hunks: 2 };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type": "feedbackSent", "worktreePath": "/wt/x", "sessionId": "session-1", "hunks": 2})
    );
}

#[test]
fn session_facts_list_mcp_servers_with_their_state() {
    use switchyard_schema::{McpServerFact, McpState, SessionFacts};
    let facts = SessionFacts {
        mcp_servers: vec![McpServerFact {
            name: "linear".into(),
            transport: "http · mcp.linear.app".into(),
            state: McpState::NeedsAuth,
            error: None,
        }],
        ..SessionFacts::default()
    };
    assert_eq!(
        serde_json::to_value(&facts).unwrap()["mcpServers"],
        json!([{"name": "linear", "transport": "http · mcp.linear.app", "state": "needsAuth", "error": null}])
    );
    // Facts stored before MCP servers existed still read.
    let old: SessionFacts = serde_json::from_value(json!({"model": null, "cliVersion": null, "cliPermissionMode": null, "turns": 0})).unwrap();
    assert!(old.mcp_servers.is_empty());
}

#[test]
fn model_prefill_is_read_per_agent_and_repository() {
    use switchyard_schema::{AgentKind, ModelChoice};
    let command: Command =
        serde_json::from_value(json!({"type": "readModelPrefill", "agent": "claudeCode", "repositoryId": "repo-1"})).unwrap();
    assert_eq!(command, Command::ReadModelPrefill { agent: AgentKind::ClaudeCode, repository_id: "repo-1".into() });
    let event = Event::ModelPrefill {
        agent: AgentKind::ClaudeCode,
        repository_id: "repo-1".into(),
        choice: ModelChoice::Default,
        retired: Some("opus".into()),
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({"type": "modelPrefill", "agent": "claudeCode", "repositoryId": "repo-1", "choice": {"type": "default"}, "retired": "opus"})
    );
}

/// `command_name` lists exactly the serde tags of `Command`: serde's "unknown variant"
/// error names every tag it expects, and each listed name is one of them.
#[test]
fn command_names_are_the_wire_tags() {
    use switchyard_schema::command_name;
    let err = serde_json::from_value::<Command>(json!({ "type": "noSuchCommand" })).unwrap_err().to_string();
    let expected = err.split("expected one of ").nth(1).expect("serde lists the variants");
    let mut tags: Vec<String> = expected.split(", ").map(|t| t.trim().trim_matches('`').to_string()).collect();
    tags.sort();
    let mut names: Vec<String> = command_name::ALL.iter().map(|n| n.to_string()).collect();
    names.sort();
    assert_eq!(names, tags);
    // And name() agrees with serde for a command with fields.
    let command = Command::OpenPullRequest { worktree_path: "/wt".into() };
    assert_eq!(serde_json::to_value(&command).unwrap()["type"], command.name());
    assert_eq!(Command::Attach.name(), "attach");
}

#[test]
fn a_model_switch_on_the_wire() {
    let set: Command = serde_json::from_value(json!({"type": "setModel", "sessionId": "session-1", "model": "claude-sonnet-5-5"})).unwrap();
    assert_eq!(set, Command::SetModel { session_id: "session-1".into(), model: "claude-sonnet-5-5".into() });
    let changed = Event::SessionModelChanged {
        session_id: "session-1".into(),
        pending_model: Some("claude-sonnet-5-5".into()),
        switched_model: None,
    };
    assert_eq!(
        serde_json::to_value(changed).unwrap(),
        json!({"type": "sessionModelChanged", "sessionId": "session-1", "pendingModel": "claude-sonnet-5-5", "switchedModel": null})
    );
    let failed = Event::ModelSwitchFailed { session_id: "session-1".into(), model: "x".into(), reason: None };
    assert_eq!(
        serde_json::to_value(failed).unwrap(),
        json!({"type": "modelSwitchFailed", "sessionId": "session-1", "model": "x", "reason": null})
    );
}

#[test]
fn a_session_stored_before_model_switching_has_no_switch() {
    let session: switchyard_schema::Session = serde_json::from_value(json!({
        "id": "session-1", "repositoryId": "repo-1", "worktreePath": "/wt", "branch": null, "agent": "claudeCode",
        "permissionMode": "askFirst", "status": "idle", "startedAtMs": 5
    }))
    .unwrap();
    assert_eq!((session.switched_model, session.pending_model), (None, None));
}
