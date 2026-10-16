//! Spend accounting through seam 1: every token_usage an agent reports is recorded as the
//! session's running spend; CLI-reported cost is passed through, a token-only CLI's cost
//! is estimated from the price table at read time.
mod harness;

use std::path::PathBuf;

use harness::{added_repository, recorded_events, started_session, Harness, TempRepo, NOW_MS};
use switchyard_schema::{Command, Event, ModelSpend, Spend};

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
                    by_model: vec![ModelSpend {
                        model: Some("claude-opus-5-5".into()),
                        input_tokens: 16,
                        output_tokens: 220,
                        cache_read_tokens: 11800,
                        cache_write_tokens: 2100,
                        reported_usd: Some(0.0412),
                        estimated_usd: None,
                        price_table_version: None,
                    }],
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
                    by_model: vec![ModelSpend {
                        model: Some("claude-opus-5-5".into()),
                        input_tokens: 40,
                        output_tokens: 310,
                        cache_read_tokens: 24100,
                        cache_write_tokens: 2100,
                        reported_usd: Some(0.0587),
                        estimated_usd: None,
                        price_table_version: None,
                    }],
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
        model: None,
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

const TWO_MODELS: &str = r#"{
  "version": "test-4",
  "models": {
    "echo-model-1": { "input": 2.0, "output": 10.0, "cacheRead": 0.5 },
    "big-model": { "input": 10.0, "output": 40.0, "cacheRead": 1.0 }
  }
}"#;

fn on(model: &str, input_tokens: u64, output_tokens: u64, cache_read_tokens: u64) -> adapter_protocol::Event {
    adapter_protocol::Event::TokenUsage {
        model: Some(model.into()),
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens: 0,
        cost_usd: None,
    }
}

/// (Model, reported, estimated in cents, input tokens) of each Model in `spend`, in order.
fn by_model(spend: &Spend) -> Vec<(Option<String>, Option<f64>, Option<f64>, u64)> {
    spend.by_model.iter().map(|m| (m.model.clone(), m.reported_usd, m.estimated_usd.map(cents), m.input_tokens)).collect()
}

#[test]
fn a_session_on_two_priced_models_is_priced_per_model_with_a_breakdown() {
    let (mut core, _repo, worktree) = setup();
    write_price_table(&core, TWO_MODELS);
    let id = started_session(&core.start_session(&worktree, "Rename the helper."));

    // A dated release priced by its family, then a second Model: 100k*2 + 20k*10 + 400k*0.5
    // = 60 cents; 10k*10 + 5k*40 = 30 cents.
    core.agent_says(0, on("echo-model-1-20261001", 100_000, 20_000, 400_000));
    let events = core.agent_says(0, on("big-model", 10_000, 5_000, 0));
    let spend = spends(&events).pop().unwrap().2;
    assert_eq!(spend.estimated_usd.map(cents), Some(90.0));
    assert_eq!((spend.input_tokens, spend.output_tokens, spend.cache_read_tokens), (110_000, 25_000, 400_000));
    assert_eq!(spend.price_table_version.as_deref(), Some("test-4"));
    assert_eq!(
        by_model(&spend),
        vec![
            (Some("echo-model-1-20261001".into()), None, Some(60.0), 100_000),
            (Some("big-model".into()), None, Some(30.0), 10_000),
        ]
    );

    // Each report is a running total for its own Model: 300k*2 + 50k*10 + 1M*0.5 = 160 cents.
    let events = core.agent_says(0, on("echo-model-1-20261001", 300_000, 50_000, 1_000_000));
    let spend = spends(&events).pop().unwrap().2;
    assert_eq!(spend.estimated_usd.map(cents), Some(190.0));
    assert_eq!(
        by_model(&spend),
        vec![
            (Some("echo-model-1-20261001".into()), None, Some(160.0), 300_000),
            (Some("big-model".into()), None, Some(30.0), 10_000),
        ]
    );

    // Kept per Model across a relaunch.
    let mut core = core.relaunch();
    let replay = spends(&core.send(Command::Attach));
    let (session, _, last) = replay.last().unwrap();
    assert_eq!(session, &id);
    assert_eq!(by_model(last), by_model(&spend));
    assert_eq!(last.estimated_usd.map(cents), Some(190.0));
}

#[test]
fn tokens_on_a_model_with_no_price_are_unpriced_and_add_nothing_to_the_priced_total() {
    let (mut core, _repo, worktree) = setup();
    write_price_table(&core, TWO_MODELS);
    started_session(&core.start_session(&worktree, "Rename the helper."));

    core.agent_says(0, on("big-model", 10_000, 5_000, 0));
    let events = core.agent_says(0, on("mystery-model", 7_000, 300, 0));

    let spend = spends(&events).pop().unwrap().2;
    assert_eq!(spend.estimated_usd.map(cents), Some(30.0));
    assert_eq!(spend.reported_usd, None);
    assert_eq!(
        by_model(&spend),
        vec![(Some("big-model".into()), None, Some(30.0), 10_000), (Some("mystery-model".into()), None, None, 7_000)]
    );
    let mystery = &spend.by_model[1];
    assert_eq!((mystery.output_tokens, mystery.price_table_version.clone()), (300, None));
}

#[test]
fn claude_codes_subagent_model_is_reported_under_its_own_model_with_its_own_cost() {
    let (mut core, _repo, worktree) = setup();
    started_session(&core.start_session(&worktree, "Fix the broken links in docs."));

    let events = core.agent_replays(0, &recorded_events("claude-code/subagent-model"));

    let spend = spends(&events).pop().unwrap().2;
    assert_eq!(spend.reported_usd.map(cents), Some(5.18));
    assert_eq!(spend.estimated_usd, None);
    assert_eq!(
        by_model(&spend),
        vec![
            (Some("claude-opus-5-5".into()), Some(0.0431), None, 12),
            (Some("claude-haiku-4-5-20251001".into()), Some(0.0087), None, 2_400),
        ]
    );
}

#[test]
fn usage_reported_without_a_model_is_the_sessions_model() {
    let (mut core, _repo, worktree) = setup();
    write_price_table(&core, TABLE_V1);
    started_session(&core.start_session(&worktree, "Rename the helper."));

    // echo's token-only CLI reports usage as 0.1 did, with no Model: priced at the
    // Session's `echo-model-1-20261001`, and named after it in the breakdown.
    let events = core.agent_replays(0, &recorded_events("echo/token-only-session"));

    let spend = spends(&events).pop().unwrap().2;
    assert_eq!(by_model(&spend), vec![(Some("echo-model-1-20261001".into()), None, Some(160.0), 300_000)]);
}
