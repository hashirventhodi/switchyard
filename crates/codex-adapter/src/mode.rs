//! Permission modes as Codex approval policy + sandbox, and back.
//!
//! | Switchyard | approval policy | sandbox |
//! | --- | --- | --- |
//! | Explore | on-request | read-only |
//! | Ask first | untrusted | workspace-write |
//! | Auto edit | on-request | workspace-write |
//! | Full auto | never | workspace-write |
//! | bypass (Full auto, repository declares a sandbox) | never | danger-full-access |
//!
//! danger-full-access is never asked for in any other mode.

use adapter_protocol::PermissionMode;
use serde_json::{json, Value};

/// Approval policy and sandbox mode for `thread/start` / `thread/resume`.
pub fn thread_policy(mode: PermissionMode) -> (&'static str, &'static str) {
    match mode {
        PermissionMode::Explore => ("on-request", "read-only"),
        PermissionMode::AskFirst => ("untrusted", "workspace-write"),
        PermissionMode::AutoEdit => ("on-request", "workspace-write"),
        PermissionMode::FullAuto => ("never", "workspace-write"),
        PermissionMode::Bypass => ("never", "danger-full-access"),
    }
}

/// Approval policy and sandbox policy for a mode switch carried on `turn/start`.
pub fn turn_policy(mode: PermissionMode) -> (Value, Value) {
    let (approval, sandbox) = thread_policy(mode);
    let sandbox = match sandbox {
        "read-only" => json!({ "type": "readOnly", "networkAccess": false }),
        "danger-full-access" => json!({ "type": "dangerFullAccess" }),
        _ => json!({
            "type": "workspaceWrite",
            "writableRoots": [],
            "networkAccess": false,
            "excludeTmpdirEnvVar": false,
            "excludeSlashTmp": false,
        }),
    };
    (json!(approval), sandbox)
}

/// The mode Codex actually applied (from a thread result's `approvalPolicy` and `sandbox`
/// policy), and Codex's own name for it: `untrusted / workspace-write`.
pub fn read_back(approval: Option<&Value>, sandbox: Option<&Value>) -> (PermissionMode, String) {
    let approval = match approval {
        Some(Value::String(name)) => name.clone(),
        Some(Value::Object(map)) if map.contains_key("granular") => "granular".into(),
        _ => "unknown".into(),
    };
    let sandbox = match sandbox.and_then(|s| s.get("type")).and_then(Value::as_str) {
        Some("readOnly") => "read-only",
        Some("workspaceWrite") => "workspace-write",
        Some("dangerFullAccess") => "danger-full-access",
        Some("externalSandbox") => "external-sandbox",
        _ => "unknown",
    };
    let mode = match (sandbox, approval.as_str()) {
        ("read-only", _) => PermissionMode::Explore,
        ("danger-full-access", _) => PermissionMode::Bypass,
        (_, "never") => PermissionMode::FullAuto,
        (_, "on-request") => PermissionMode::AutoEdit,
        // untrusted, granular, anything newer: it asks at least as often as Ask first.
        _ => PermissionMode::AskFirst,
    };
    (mode, format!("{approval} / {sandbox}"))
}
