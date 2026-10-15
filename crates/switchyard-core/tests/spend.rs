//! Spend accounting through seam 1: every token_usage an agent reports is recorded as the
//! session's running spend; CLI-reported cost is passed through, a token-only CLI's cost
//! is estimated from the price table at read time.
mod harness;

use std::path::PathBuf;

use harness::{added_repository, recorded_events, started_session, Harness, TempRepo, NOW_MS};
use switchyard_schema::{Command, Event, Spend};

fn setup() -> (Harness, TempRepo, PathBuf) {
    let repo = TempRepo::new("acme");
    let worktree = repo.add_worktree("fix/readme");
    let mut core = Harness::launch();
    added_repository(&core.add_repository(&repo.path));
    (core, repo, worktree)
}

/// Every SpendRecorded in `events`, as (session id, at, spend).
fn spends(events: &[Event]) -> Vec<(String, u64, Spend)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::SpendRecorded { session_id, at_ms, spend } => Some((session_id.clone(), *at_ms, spend.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn claude_code_reported_cost_is_recorded_as_reported_with_its_running_token_totals() {
    let (mut core, _repo, worktree) = setup();
    let id = started_session(&core.start_session(&worktree, "Fix the broken link in the README."));

    let events = core.agent_replays(0, &recorded_events("claude-code/first-session"));

    // token_usage carries running totals: each one replaces the last, never adds to it.
    assert_eq!(
        spends(&events),
        vec![
            (
                id.clone(),
                NOW_MS,
                Spend {
                    input_tokens: 16,
                    output_tokens: 220,
                    cache_read_tokens: 11800,
                    cache_write_tokens: 2100,
                    reported_usd: Some(0.0412),
                    estimated_usd: None,
                    price_table_version: None,
                }
            ),
            (
                id,
                NOW_MS,
                Spend {
                    input_tokens: 40,
                    output_tokens: 310,
                    cache_read_tokens: 24100,
                    cache_write_tokens: 2100,
                    reported_usd: Some(0.0587),
                    estimated_usd: None,
                    price_table_version: None,
                }
            ),
        ]
    );
}

const TABLE_V1: &str = r#"{
  "version": "test-1",
  "models": {
    "echo-model-1": { "input": 2.0, "output": 10.0, "cacheRead": 0.5 },
    "echo-model-10": { "input": 99.0, "output": 99.0, "cacheRead": 99.0 }
  }
}"#;

fn write_price_table(core: &Harness, json: &str) {
    std::fs::write(core.data_dir().join("price-table.json"), json).unwrap();
}

/// (reported, estimated in cents, price table version) of each SpendRecorded in `events`.
fn costs(events: &[Event]) -> Vec<(Option<f64>, Option<f64>, Option<String>)> {
    spends(events)
        .into_iter()
        .map(|(_, _, s)| (s.reported_usd, s.estimated_usd.map(cents), s.price_table_version))
        .collect()
}

/// USD to cents, rounded to a hundredth of a cent (float noise aside).
fn cents(usd: f64) -> f64 {
    (usd * 10_000.0).round() / 100.0
}

#[test]
fn a_token_only_cli_gets_its_cost_estimated_from_the_price_table() {
    let (mut core, _repo, worktree) = setup();
    write_price_table(&core, TABLE_V1);
    started_session(&core.start_session(&worktree, "Rename the helper."));

    let events = core.agent_replays(0, &recorded_events("echo/token-only-session"));

    // The model `echo-model-1-20261001` is priced by its dated family `echo-model-1`
    // (and not by `echo-model-10`). USD per million tokens:
    //   100k in * 2 + 20k out * 10 + 400k cache * 0.5 = 0.2 + 0.2 + 0.2
    //   300k in * 2 + 50k out * 10 + 1M cache * 0.5   = 0.6 + 0.5 + 0.5
    let v1 = Some("test-1".to_string());
    assert_eq!(costs(&events), vec![(None, Some(60.0), v1.clone()), (None, Some(160.0), v1)]);
}

#[test]
fn editing_the_price_table_changes_earlier_estimates_when_they_are_next_read() {
    let (mut core, _repo, worktree) = setup();
    write_price_table(&core, TABLE_V1);
    let id = started_session(&core.start_session(&worktree, "Rename the helper."));
    core.agent_replays(0, &recorded_events("echo/token-only-session"));

    write_price_table(
        &core,
        r#"{"version": "test-2", "models": {"echo-model-1": {"input": 1.0, "output": 5.0, "cacheRead": 0.25}}}"#,
    );
    let replay = core.send(Command::Attach);

    // Half the v1 prices: the stored tokens are re-priced, nothing is migrated.
    let v2 = Some("test-2".to_string());
    assert_eq!(costs(&replay), vec![(None, Some(30.0), v2.clone()), (None, Some(80.0), v2)]);
    assert!(spends(&replay).iter().all(|(session, at, _)| *session == id && *at == NOW_MS));
}

#[test]
fn a_model_missing_from_the_price_table_records_tokens_without_an_estimate() {
    let (mut core, _repo, worktree) = setup();
    write_price_table(&core, r#"{"version": "t", "models": {"other-model": {"input": 1.0, "output": 1.0, "cacheRead": 1.0}}}"#);
    started_session(&core.start_session(&worktree, "Rename the helper."));

    let events = core.agent_replays(0, &recorded_events("echo/token-only-session"));

    let last = spends(&events).pop().unwrap().2;
    assert_eq!((last.input_tokens, last.output_tokens, last.cache_read_tokens), (300_000, 50_000, 1_000_000));
    assert_eq!((last.reported_usd, last.estimated_usd, last.price_table_version), (None, None, None));
}

#[test]
fn cache_writes_are_recorded_priced_and_kept_across_a_relaunch() {
    let (mut core, _repo, worktree) = setup();
    write_price_table(&core, TABLE_V1);
    started_session(&core.start_session(&worktree, "Rename the helper."));
    core.agent_replays(0, &recorded_events("echo/token-only-session"));

    let usage = |cache_write_tokens| adapter_protocol::Event::TokenUsage {
        input_tokens: 300_000,
        output_tokens: 50_000,
        cache_read_tokens: 1_000_000,
        cache_write_tokens,
        cost_usd: None,
    };
    // No cacheWrite price in the table: a cache write costs what input costs.
    //   160 cents as before + 400k cache writes * 2.0 = 240 cents
    let events = core.agent_says(0, usage(400_000));
    assert_eq!(spends(&events)[0].2.cache_write_tokens, 400_000);
    assert_eq!(costs(&events), vec![(None, Some(240.0), Some("test-1".into()))]);

    write_price_table(
        &core,
        r#"{"version": "test-3", "models": {"echo-model-1": {"input": 2.0, "output": 10.0, "cacheRead": 0.5, "cacheWrite": 2.5}}}"#,
    );
    let mut core = core.relaunch();
    let replay = core.send(Command::Attach);
    let last = spends(&replay).pop().unwrap().2;
    assert_eq!(last.cache_write_tokens, 400_000, "stored raw and replayed");
    //   160 + 400k * 2.5 = 260 cents
    assert_eq!(last.estimated_usd.map(cents), Some(260.0));
}
