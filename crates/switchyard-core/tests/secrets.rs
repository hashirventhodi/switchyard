//! Secrets grants through seam 1: a Repository's secrets are listed by name with grant /
//! revoke; values live in the OS keychain (fake here) or the environment, never in the
//! store; a granted secret reaches the spawn's environment, an ungranted one is kept out.
mod harness;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use harness::{added_repository, Harness, TempRepo};
use adapter_protocol::Event as AgentEvent;
use switchyard_schema::{Command, Entry, Event, Secret, SecretOrigin, SecretSource};

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
fn the_session_environment_has_the_granted_secrets_and_none_of_the_others() {
    let mut s = setup(Some(POLICY), Some(MCP));
    s.core.secrets.in_keychain("NPM_TOKEN", "npm-123");
    s.core.secrets.in_environment("NPM_TOKEN", "npm-from-env");
    s.core.secrets.in_environment("GITHUB_TOKEN", "ghp-456");
    s.core.secrets.in_environment("SENTRY_DSN", "https://sentry");
    grant(&mut s, "NPM_TOKEN");
    grant(&mut s, "SENTRY_DSN");

    let worktree = s.worktree.clone();
    harness::started_session(&s.core.start_session(&worktree, "Ship it."));

    let env = &s.core.agents.launches()[0].env;
    assert_eq!(
        env.set,
        BTreeMap::from([("NPM_TOKEN".into(), "npm-123".into()), ("SENTRY_DSN".into(), "https://sentry".into())])
    );
    assert_eq!(env.remove, BTreeSet::from(["GITHUB_TOKEN".to_string()]));
}

#[test]
fn a_secret_granted_to_another_repository_is_kept_out() {
    let mut s = setup(None, None);
    let other = TempRepo::new("billing");
    std::fs::create_dir_all(other.path.join(".switchyard")).unwrap();
    std::fs::write(other.path.join(".switchyard/policy.yml"), "secrets: [STRIPE_KEY]\n").unwrap();
    let other_id = added_repository(&s.core.add_repository(&other.path)).id;
    s.core.secrets.in_environment("STRIPE_KEY", "sk-789");
    s.core.send(Command::GrantSecret { repository_id: other_id, name: "STRIPE_KEY".into() });

    let worktree = s.worktree.clone();
    harness::started_session(&s.core.start_session(&worktree, "Ship it."));

    let env = &s.core.agents.launches()[0].env;
    assert!(env.set.is_empty(), "{env:?}");
    assert_eq!(env.remove, BTreeSet::from(["STRIPE_KEY".to_string()]));
}

#[test]
fn revoking_is_noted_in_running_sessions_and_takes_effect_at_the_next_spawn() {
    let mut s = setup(Some(POLICY), None);
    s.core.secrets.in_keychain("NPM_TOKEN", "npm-123");
    grant(&mut s, "NPM_TOKEN");
    let worktree = s.worktree.clone();
    let id = harness::started_session(&s.core.start_session(&worktree, "Ship it."));
    s.core.agent_says(0, init());
    s.core.agent_says(0, AgentEvent::TurnComplete);

    let events = revoke(&mut s, "NPM_TOKEN");

    let note = events.iter().find_map(|e| match e {
        Event::TranscriptEntry { session_id, entry: Entry::System { text }, .. } if *session_id == id => Some(text.clone()),
        _ => None,
    });
    assert_eq!(
        note.as_deref(),
        Some("NPM_TOKEN was revoked for this repository. The running agent keeps it until it is next started.")
    );
    assert_eq!(granted_names(&listed(&events)), Vec::<String>::new());

    // The agent process ends; talking to it again starts it without the secret.
    s.core.agent_exits(0, Some(0), "");
    s.core.send(Command::SendPrompt { session_id: id, text: "Go on.".into() });
    let relaunch = &s.core.agents.launches()[1].env;
    assert!(relaunch.set.is_empty(), "{relaunch:?}");
    assert_eq!(relaunch.remove, BTreeSet::from(["NPM_TOKEN".to_string(), "SENTRY_DSN".to_string()]));
}

fn init() -> AgentEvent {
    AgentEvent::SessionInit {
        external_session_id: "ext-1".into(),
        permission_mode: adapter_protocol::PermissionMode::AskFirst,
        cli_permission_mode: "default".into(),
        cli: adapter_protocol::CliInfo { name: "claude-code".into(), version: "2.1.300".into() },
        model: None,
        mcp_servers: vec![],
    }
}

#[test]
fn a_value_set_from_context_goes_to_the_keychain_and_never_into_the_store_or_events() {
    let mut s = setup(None, None);
    let repository_id = s.repo_id.clone();
    let added = s.core.send(Command::AddSecret { repository_id: repository_id.clone(), name: "STRIPE_KEY".into() });
    assert_eq!(listed(&added), vec![secret("STRIPE_KEY", false, SecretOrigin::Local, SecretSource::Missing)]);

    let events = s.core.send(Command::SetSecretValue {
        repository_id: repository_id.clone(),
        name: "STRIPE_KEY".into(),
        value: "sk-live-0123456789".into(),
    });
    grant(&mut s, "STRIPE_KEY");

    assert_eq!(s.core.secrets.keychain("STRIPE_KEY").as_deref(), Some("sk-live-0123456789"));
    assert_eq!(listed(&events), vec![secret("STRIPE_KEY", false, SecretOrigin::Local, SecretSource::Keychain)]);
    assert!(!format!("{events:?}").contains("sk-live"), "{events:#?}");
    let worktree = s.worktree.clone();
    harness::started_session(&s.core.start_session(&worktree, "Ship it."));
    assert!(!format!("{:?}", s.core.agents.launches()[0]).contains("sk-live"), "Debug of a launch shows no value");
    // Nothing the core wrote on disk holds the value.
    for file in walk(s.core.data_dir()) {
        let bytes = std::fs::read(&file).unwrap();
        assert!(!bytes.windows(10).any(|w| w == b"sk-live-01"), "{} holds the value", file.display());
    }
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn a_locally_added_secret_can_be_removed_but_a_declared_one_stays() {
    let mut s = setup(Some(POLICY), None);
    let repository_id = s.repo_id.clone();
    s.core.send(Command::AddSecret { repository_id: repository_id.clone(), name: "STRIPE_KEY".into() });
    grant(&mut s, "STRIPE_KEY");

    let removed = s.core.send(Command::RemoveSecret { repository_id: repository_id.clone(), name: "STRIPE_KEY".into() });
    assert_eq!(listed(&removed).iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["NPM_TOKEN", "SENTRY_DSN"]);

    let refused = s.core.send(Command::RemoveSecret { repository_id, name: "NPM_TOKEN".into() });
    assert!(
        refused.iter().any(|e| matches!(e, Event::CommandRefused { reason, .. } if reason.contains("policy"))),
        "{refused:#?}"
    );
}

#[test]
fn a_secret_name_must_be_an_environment_variable_name() {
    let mut s = setup(None, None);
    let events = s.core.send(Command::AddSecret { repository_id: s.repo_id.clone(), name: "not a name".into() });
    assert!(events.iter().any(|e| matches!(e, Event::CommandRefused { .. })), "{events:#?}");
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
