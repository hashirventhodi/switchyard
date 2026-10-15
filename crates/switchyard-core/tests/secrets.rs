//! Secrets grants through seam 1: a Repository's secrets are listed by name with grant /
//! revoke; values live in the OS keychain (fake here) or the environment, never in the
//! store; a granted secret reaches the spawn's environment, an ungranted one is kept out.
mod harness;

use std::path::PathBuf;

use harness::{added_repository, Harness, TempRepo};
use switchyard_schema::{Command, Event, Secret, SecretOrigin, SecretSource};

struct Setup {
    core: Harness,
    repo: TempRepo,
    repo_id: String,
    worktree: PathBuf,
}

fn setup(policy: Option<&str>, mcp_json: Option<&str>) -> Setup {
    let repo = TempRepo::new("acme");
    if let Some(text) = policy {
        std::fs::create_dir_all(repo.path.join(".switchyard")).unwrap();
        std::fs::write(repo.path.join(".switchyard/policy.yml"), text).unwrap();
    }
    if let Some(text) = mcp_json {
        std::fs::write(repo.path.join(".mcp.json"), text).unwrap();
    }
    let worktree = repo.add_worktree("feat/secrets");
    let mut core = Harness::launch();
    let repo_id = added_repository(&core.add_repository(&repo.path)).id;
    Setup { core, repo, repo_id, worktree }
}

const POLICY: &str = "secrets:\n  - NPM_TOKEN\n  - SENTRY_DSN\n";

const MCP: &str = r#"{"mcpServers":{"github":{"command":"npx","args":["-y","server-github"],"env":{"GITHUB_TOKEN":"${GITHUB_TOKEN}"}}}}"#;

fn listed(events: &[Event]) -> Vec<Secret> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SecretsChanged { secrets, .. } => Some(secrets.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no secretsChanged in {events:#?}"))
}

fn secret(name: &str, granted: bool, origin: SecretOrigin, source: SecretSource) -> Secret {
    Secret { name: name.into(), granted, origin, source }
}

#[test]
fn context_lists_the_repositorys_secrets_by_name_with_where_each_value_lives() {
    let mut s = setup(Some(POLICY), Some(MCP));
    s.core.secrets.in_keychain("NPM_TOKEN", "npm-123");
    s.core.secrets.in_environment("GITHUB_TOKEN", "ghp-456");

    let events = s.core.send(Command::ReadSecrets { repository_id: s.repo_id.clone() });

    assert_eq!(
        listed(&events),
        vec![
            secret("GITHUB_TOKEN", false, SecretOrigin::Mcp, SecretSource::Environment),
            secret("NPM_TOKEN", false, SecretOrigin::Policy, SecretSource::Keychain),
            secret("SENTRY_DSN", false, SecretOrigin::Policy, SecretSource::Missing),
        ]
    );
}

fn granted_names(secrets: &[Secret]) -> Vec<String> {
    secrets.iter().filter(|x| x.granted).map(|x| x.name.clone()).collect()
}

fn grant(s: &mut Setup, name: &str) -> Vec<Event> {
    s.core.send(Command::GrantSecret { repository_id: s.repo_id.clone(), name: name.into() })
}

fn revoke(s: &mut Setup, name: &str) -> Vec<Event> {
    s.core.send(Command::RevokeSecret { repository_id: s.repo_id.clone(), name: name.into() })
}

#[test]
fn granting_and_revoking_show_in_context_at_once_and_survive_a_relaunch() {
    let mut s = setup(Some(POLICY), None);

    assert_eq!(granted_names(&listed(&grant(&mut s, "NPM_TOKEN"))), ["NPM_TOKEN"]);
    grant(&mut s, "SENTRY_DSN");
    assert_eq!(granted_names(&listed(&revoke(&mut s, "SENTRY_DSN"))), ["NPM_TOKEN"]);

    let Setup { core, repo_id, repo: _repo, .. } = s;
    let mut core = core.relaunch();
    let after = listed(&core.send(Command::ReadSecrets { repository_id: repo_id }));
    assert_eq!(granted_names(&after), ["NPM_TOKEN"]);
}

#[test]
fn a_secret_the_repository_does_not_list_cannot_be_granted() {
    let mut s = setup(Some(POLICY), None);
    let events = grant(&mut s, "AWS_SECRET");
    assert!(
        events.iter().any(|e| matches!(e, Event::CommandRefused { reason, .. } if reason.contains("AWS_SECRET"))),
        "{events:#?}"
    );
}
