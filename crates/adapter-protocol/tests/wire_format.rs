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
fn protocol_version_is_0_1_0() {
    assert_eq!(PROTOCOL_VERSION.to_string(), "0.1.0");
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
        json!({ "type": "token_usage", "input_tokens": 10, "output_tokens": 5, "cache_read_tokens": 2, "cost_usd": 0.01 }),
        Event::TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 2,
            cost_usd: Some(0.01),
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
        json!({ "type": "resume", "external_session_id": "sess-1" }),
        Command::Resume {
            external_session_id: "sess-1".into(),
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
