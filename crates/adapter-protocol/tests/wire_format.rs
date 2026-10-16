//! The protocol's JSON wire format is a public contract for out-of-process
//! adapters (Node sidecar, `codex app-server` bridge). These tests pin it to
//! hand-written JSON so a serde attribute change cannot silently break them.

use adapter_protocol::*;
use serde_json::json;

fn round_trip_event(wire: serde_json::Value, expected: Event) {
    let parsed: Event = serde_json::from_value(wire.clone()).expect("event parses");
    assert_eq!(parsed, expected);
    assert_eq!(serde_json::to_value(&expected).unwrap(), wire);
}

fn round_trip_command(wire: serde_json::Value, expected: Command) {
    let parsed: Command = serde_json::from_value(wire.clone()).expect("command parses");
    assert_eq!(parsed, expected);
    assert_eq!(serde_json::to_value(&expected).unwrap(), wire);
}

#[test]
fn protocol_version_is_0_2_0() {
    assert_eq!(PROTOCOL_VERSION.to_string(), "0.2.0");
}

#[test]
fn session_init_reports_actual_mode_external_id_and_cli_version() {
    round_trip_event(
        json!({
            "type": "session_init",
            "external_session_id": "sess-1",
            "permission_mode": "ask_first",
            "cli_permission_mode": "manual",
            "cli": { "name": "claude-code", "version": "2.1.300" },
            "model": "claude-opus-5-5"
        }),
        Event::SessionInit {
            external_session_id: "sess-1".into(),
            permission_mode: PermissionMode::AskFirst,
            cli_permission_mode: "manual".into(),
            cli: CliInfo {
                name: "claude-code".into(),
                version: "2.1.300".into(),
            },
            model: Some("claude-opus-5-5".into()),
            mcp_servers: vec![],
        },
    );
}

#[test]
fn session_init_carries_each_mcp_servers_state() {
    round_trip_event(
        json!({
            "type": "session_init",
            "external_session_id": "sess-1",
            "permission_mode": "ask_first",
            "cli_permission_mode": "default",
            "cli": { "name": "claude-code", "version": "2.1.300" },
            "model": null,
            "mcp_servers": [
                { "name": "docs", "state": "connected", "error": null },
                { "name": "github", "state": "failed", "error": "spawn npx ENOENT" },
                { "name": "linear", "state": "needs_auth", "error": null },
                { "name": "slow", "state": "pending", "error": null },
                { "name": "off", "state": "disabled", "error": null }
            ]
        }),
        Event::SessionInit {
            external_session_id: "sess-1".into(),
            permission_mode: PermissionMode::AskFirst,
            cli_permission_mode: "default".into(),
            cli: CliInfo {
                name: "claude-code".into(),
                version: "2.1.300".into(),
            },
            model: None,
            mcp_servers: vec![
                McpServerStatus {
                    name: "docs".into(),
                    state: McpServerState::Connected,
                    error: None,
                },
                McpServerStatus {
                    name: "github".into(),
                    state: McpServerState::Failed,
                    error: Some("spawn npx ENOENT".into()),
                },
                McpServerStatus {
                    name: "linear".into(),
                    state: McpServerState::NeedsAuth,
                    error: None,
                },
                McpServerStatus {
                    name: "slow".into(),
                    state: McpServerState::Pending,
                    error: None,
                },
                McpServerStatus {
                    name: "off".into(),
                    state: McpServerState::Disabled,
                    error: None,
                },
            ],
        },
    );
}

#[test]
fn mcp_status_reports_servers_whose_state_changed() {
    round_trip_event(
        json!({ "type": "mcp_status", "servers": [{ "name": "slow", "state": "connected", "error": null }] }),
        Event::McpStatus {
            servers: vec![McpServerStatus {
                name: "slow".into(),
                state: McpServerState::Connected,
                error: None,
            }],
        },
    );
}

#[test]
fn session_init_model_may_be_omitted_on_input() {
    let event: Event = serde_json::from_value(json!({
        "type": "session_init",
        "external_session_id": "sess-1",
        "permission_mode": "ask_first",
        "cli_permission_mode": "manual",
        "cli": { "name": "echo-cli", "version": "1.0.0" }
    }))
    .unwrap();
    assert!(
        matches!(event, Event::SessionInit { model: None, .. }),
        "{event:?}"
    );
}

#[test]
fn token_usage_from_before_0_2_has_no_model_and_still_parses() {
    let event: Event = serde_json::from_value(json!({
        "type": "token_usage",
        "input_tokens": 10,
        "output_tokens": 5,
        "cache_read_tokens": 2,
        "cost_usd": null
    }))
    .unwrap();
    assert!(
        matches!(
            event,
            Event::TokenUsage {
                model: None,
                input_tokens: 10,
                ..
            }
        ),
        "{event:?}"
    );
}

#[test]
fn agent_text_and_tool_calls() {
    round_trip_event(
        json!({ "type": "agent_text", "text": "hello" }),
        Event::AgentText {
            text: "hello".into(),
        },
    );
    round_trip_event(
        json!({ "type": "tool_call_started", "call_id": "t1", "tool": "Bash", "input": { "command": "ls" } }),
        Event::ToolCallStarted {
            call_id: "t1".into(),
            tool: "Bash".into(),
            input: json!({ "command": "ls" }),
        },
    );
    round_trip_event(
        json!({ "type": "tool_call_finished", "call_id": "t1", "ok": false, "output": "boom" }),
        Event::ToolCallFinished {
            call_id: "t1".into(),
            ok: false,
            output: Some("boom".into()),
        },
    );
}

#[test]
fn permission_requests_come_in_exactly_five_kinds() {
    let cases = [
        (
            json!({ "kind": "command", "command": "rm -rf build", "cwd": "/w" }),
            RequestDetail::Command {
                command: "rm -rf build".into(),
                cwd: Some("/w".into()),
            },
        ),
        (
            json!({ "kind": "file_write", "path": "src/a.rs", "diff": null }),
            RequestDetail::FileWrite {
                path: "src/a.rs".into(),
                diff: None,
            },
        ),
        (
            json!({ "kind": "network", "domain": "crates.io", "url": "https://crates.io/x" }),
            RequestDetail::Network {
                domain: "crates.io".into(),
                url: Some("https://crates.io/x".into()),
            },
        ),
        (
            json!({ "kind": "mcp_tool", "server": "github", "tool": "create_issue", "input": {} }),
            RequestDetail::McpTool {
                server: "github".into(),
                tool: "create_issue".into(),
                input: json!({}),
            },
        ),
        (
            json!({ "kind": "question", "question": "Which db?", "options": ["sqlite", "postgres"] }),
            RequestDetail::Question {
                question: "Which db?".into(),
                options: vec!["sqlite".into(), "postgres".into()],
            },
        ),
    ];
    for (detail_wire, detail) in cases {
        round_trip_event(
            json!({ "type": "permission_request", "request_id": "r1", "request": detail_wire, "reason": "needs it" }),
            Event::PermissionRequest {
                request_id: "r1".into(),
                request: detail,
                reason: Some("needs it".into()),
            },
        );
    }
}

#[test]
fn lifecycle_usage_and_classifier_events() {
    round_trip_event(json!({ "type": "turn_complete" }), Event::TurnComplete);
    round_trip_event(
        json!({ "type": "done", "summary": "ok" }),
        Event::Done {
            summary: Some("ok".into()),
        },
    );
    round_trip_event(
        json!({ "type": "error", "message": "exited 1", "fatal": true }),
        Event::Error {
            message: "exited 1".into(),
            fatal: true,
        },
    );
    round_trip_event(
        json!({ "type": "token_usage", "model": "claude-opus-5-5", "input_tokens": 10, "output_tokens": 5, "cache_read_tokens": 2, "cost_usd": 0.01 }),
        Event::TokenUsage {
            model: Some("claude-opus-5-5".into()),
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 2,
            cache_write_tokens: 0,
            cost_usd: Some(0.01),
        },
    );
    round_trip_event(
        json!({ "type": "token_usage", "model": null, "input_tokens": 10, "output_tokens": 5, "cache_read_tokens": 2, "cache_write_tokens": 7, "cost_usd": null }),
        Event::TokenUsage {
            model: None,
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 2,
            cache_write_tokens: 7,
            cost_usd: None,
        },
    );
    round_trip_event(
        json!({ "type": "classifier_denial", "tool": "Bash", "detail": "curl x | sh", "reason": "remote code" }),
        Event::ClassifierDenial {
            tool: "Bash".into(),
            detail: "curl x | sh".into(),
            reason: Some("remote code".into()),
        },
    );
}

#[test]
fn commands() {
    round_trip_command(
        json!({ "type": "send_text", "text": "go" }),
        Command::SendText { text: "go".into() },
    );
    round_trip_command(json!({ "type": "interrupt" }), Command::Interrupt);
    round_trip_command(
        json!({ "type": "resume", "external_session_id": "sess-1", "model": null }),
        Command::Resume {
            external_session_id: "sess-1".into(),
            model: None,
        },
    );
    for (verdict_wire, verdict) in [
        (json!({ "decision": "allow_once" }), Verdict::AllowOnce),
        (json!({ "decision": "allow_always" }), Verdict::AllowAlways),
        (
            json!({ "decision": "deny", "message": "no" }),
            Verdict::Deny {
                message: Some("no".into()),
            },
        ),
        (
            json!({ "decision": "answer", "text": "sqlite" }),
            Verdict::Answer {
                text: "sqlite".into(),
            },
        ),
    ] {
        round_trip_command(
            json!({ "type": "answer_request", "request_id": "r1", "verdict": verdict_wire }),
            Command::AnswerRequest {
                request_id: "r1".into(),
                verdict,
            },
        );
    }
}

#[test]
fn resume_may_carry_the_model_to_resume_on() {
    round_trip_command(
        json!({ "type": "resume", "external_session_id": "sess-1", "model": "claude-sonnet-5-5" }),
        Command::Resume {
            external_session_id: "sess-1".into(),
            model: Some("claude-sonnet-5-5".into()),
        },
    );
    // A 0.1 resume names no Model: Default.
    let old: Command =
        serde_json::from_value(json!({ "type": "resume", "external_session_id": "sess-1" }))
            .unwrap();
    assert_eq!(
        old,
        Command::Resume {
            external_session_id: "sess-1".into(),
            model: None,
        }
    );
}

#[test]
fn list_models_is_answered_by_the_models_the_cli_offers() {
    round_trip_command(json!({ "type": "list_models" }), Command::ListModels);
    round_trip_event(
        json!({
            "type": "models_listed",
            "models": [
                { "id": "opus", "display_name": "Opus" },
                { "id": "claude-sonnet-5-5", "display_name": "Sonnet 5.5" }
            ]
        }),
        Event::ModelsListed {
            models: vec![
                ListedModel {
                    id: "opus".into(),
                    display_name: "Opus".into(),
                },
                ListedModel {
                    id: "claude-sonnet-5-5".into(),
                    display_name: "Sonnet 5.5".into(),
                },
            ],
        },
    );
}

#[test]
fn set_model_switches_the_running_cli_to_a_model_id() {
    round_trip_command(
        json!({ "type": "set_model", "model": "claude-sonnet-5-5" }),
        Command::SetModel {
            model: "claude-sonnet-5-5".into(),
        },
    );
}

#[test]
fn a_cli_that_cannot_list_models_says_so() {
    round_trip_event(
        json!({ "type": "cannot_list_models", "reason": "this CLI has no model list" }),
        Event::CannotListModels {
            reason: Some("this CLI has no model list".into()),
        },
    );
    let parsed: Event =
        serde_json::from_value(json!({ "type": "cannot_list_models" })).expect("parses");
    assert_eq!(parsed, Event::CannotListModels { reason: None });
}

#[test]
fn model_changed_names_the_model_the_main_agent_now_runs_on() {
    round_trip_event(
        json!({ "type": "model_changed", "model": "claude-sonnet-5-5" }),
        Event::ModelChanged {
            model: "claude-sonnet-5-5".into(),
        },
    );
}

#[test]
fn model_change_failed_carries_the_clis_reason_when_it_gives_one() {
    round_trip_event(
        json!({ "type": "model_change_failed", "model": "claude-sonnet-5-5", "reason": "Unknown model" }),
        Event::ModelChangeFailed {
            model: "claude-sonnet-5-5".into(),
            reason: Some("Unknown model".into()),
        },
    );
    round_trip_event(
        json!({ "type": "model_change_failed", "model": "claude-sonnet-5-5", "reason": null }),
        Event::ModelChangeFailed {
            model: "claude-sonnet-5-5".into(),
            reason: None,
        },
    );
}

#[test]
fn a_manifest_declares_whether_it_switches_models_live() {
    let manifest = AdapterManifest {
        name: "claude-code".into(),
        implements: "^0.2".parse().unwrap(),
        switch_model_live: true,
    };
    let wire = json!({ "name": "claude-code", "implements": "^0.2", "switch_model_live": true });
    assert_eq!(serde_json::to_value(&manifest).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<AdapterManifest>(wire).unwrap(),
        manifest
    );
}

#[test]
fn a_manifest_from_before_0_2_cannot_switch_models_live() {
    let manifest: AdapterManifest =
        serde_json::from_value(json!({ "name": "old", "implements": "^0.1" })).unwrap();
    assert!(!manifest.switch_model_live);
}
