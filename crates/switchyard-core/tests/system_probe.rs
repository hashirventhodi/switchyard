//! The real agent detection runs only local programs: the CLI's `--version` and the
//! adapter's `--manifest`. Uses stand-in shell scripts (no real CLI, no Node, no network).
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use switchyard_core::compat::{AgentProbe, SystemProbe};
use switchyard_schema::AgentKind;
use tempfile::TempDir;

/// An executable shell script at `dir/name` that records its arguments in `dir/name.args`.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    let text = format!("#!/bin/sh\necho \"$@\" >> \"{}.args\"\n{body}\n", path.display());
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn args(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(format!("{name}.args"))).unwrap_or_default()
}

fn probe(search_dirs: Vec<PathBuf>, node: PathBuf) -> SystemProbe {
    SystemProbe { search_dirs, node, sidecar: PathBuf::from("/app/claude-sidecar.mjs") }
}

#[test]
fn the_cli_version_comes_from_running_it_with_version_in_a_search_dir() {
    let empty = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    script(bin.path(), "claude", "echo '2.1.300 (Claude Code)'");
    let probe = probe(vec![empty.path().into(), bin.path().into()], "node".into());

    let version = probe.cli_version(AgentKind::ClaudeCode);

    assert_eq!(version, Ok(Some("2.1.300 (Claude Code)\n".into())));
    assert_eq!(args(bin.path(), "claude"), "--version\n");
}

#[test]
fn a_cli_in_no_search_dir_is_not_installed() {
    let empty = TempDir::new().unwrap();
    let probe = probe(vec![empty.path().into()], "node".into());

    assert_eq!(probe.cli_version(AgentKind::ClaudeCode), Ok(None));
}

#[test]
fn a_cli_whose_version_run_fails_is_an_error() {
    let bin = TempDir::new().unwrap();
    script(bin.path(), "claude", "echo 'broken install' >&2; exit 4");
    let probe = probe(vec![bin.path().into()], "node".into());

    let error = probe.cli_version(AgentKind::ClaudeCode).unwrap_err();

    assert!(error.contains("broken install"), "{error}");
}

#[test]
fn the_adapter_manifest_comes_from_running_the_sidecar_with_manifest() {
    let bin = TempDir::new().unwrap();
    let node = script(bin.path(), "node", r#"echo '{"name":"claude-code","implements":"^0.2"}'"#);
    let probe = probe(vec![], node);

    let manifest = probe.adapter_manifest(AgentKind::ClaudeCode).unwrap();

    assert_eq!(manifest.name, "claude-code");
    assert_eq!(manifest.implements.to_string(), "^0.2");
    assert_eq!(args(bin.path(), "node"), "/app/claude-sidecar.mjs --manifest\n");
}

#[test]
fn an_adapter_runtime_that_cannot_run_is_an_error_naming_it() {
    let probe = probe(vec![], "/definitely/not/node".into());

    let error = probe.adapter_manifest(AgentKind::ClaudeCode).unwrap_err();

    assert!(error.contains("/definitely/not/node"), "{error}");
}
