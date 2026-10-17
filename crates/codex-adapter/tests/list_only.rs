//! A Codex adapter the host started only to list Models (`Launch::list_only`) can do nothing
//! else: it initialises Codex and asks `model/list`, never starts a thread, and refuses
//! every other command.

use adapter_protocol::{Adapter, Command, PermissionMode};
use codex_adapter::{CodexAdapter, Launch};

fn list_only() -> CodexAdapter {
    let launch: Launch = serde_json::from_value(serde_json::json!({
        "cwd": "/tmp/switchyard-models-x",
        "permission_mode": "explore",
        "list_only": true,
    }))
    .unwrap();
    assert_eq!(launch.permission_mode, PermissionMode::Explore);
    CodexAdapter::new(launch)
}

fn methods(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["method"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn a_list_only_adapter_refuses_every_command_but_list_models() {
    let mut adapter = list_only();
    let refused = [
        Command::SendText { text: "rm -rf ~".into() },
        Command::Resume { external_session_id: "019a-thread".into(), model: None },
        Command::SetModel { model: "gpt-5.6-sol".into() },
        Command::SetPermissionMode { permission_mode: PermissionMode::FullAuto },
        Command::Interrupt,
    ];
    for command in &refused {
        assert!(adapter.command(command).is_err(), "{command:?} must be refused");
    }

    let sent = adapter.command(&Command::ListModels).unwrap();

    assert_eq!(methods(&sent), vec!["initialize"], "nothing was sent before; listing starts now");
}

#[test]
fn a_list_only_adapter_never_starts_a_thread_even_after_listing() {
    let mut adapter = list_only();
    adapter.command(&Command::ListModels).unwrap();
    adapter
        .ingest(r#"{"id":1,"result":{"userAgent":"switchyard/0.144.1 (Mac OS 26.1.0; arm64)"}}"#)
        .unwrap();

    assert!(adapter.command(&Command::SendText { text: "Go.".into() }).is_err());
}
