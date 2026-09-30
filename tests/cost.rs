//! API-equivalent cost (#10).
//!
//! Seams under test (seam 1 of `tests/common`): fixture transcripts are
//! dropped into the Claude projects dir, `claudit::ingest::run` loads them,
//! and assertions are made on the typed reports of `stats::cost`, priced with
//! a `claudit::pricing::PriceTable` passed in by the caller (the built-in
//! table, or one parsed from a test TOML to show retroactive repricing).
//! Nothing here looks at tables.
//!
//! Hand computation for fixture session `8d0c5a3e` (tokens from the fixtures
//! README, prices from `pricing/prices.toml`, $/MTok = µ$/token). Every
//! cache write in the `projects/` fixtures is a 1-hour write
//! (`ephemeral_1h_input_tokens`), priced at the 1h rate:
//!
//! - main thread, claude-opus-5-5 ($4 in, $20 out, $8 1h cache write,
//!   $0.20 cache read):
//!   7×4 + 1162×20 + 15000×8 + 71300×0.20
//!   = 28 + 23240 + 120000 + 14260 = 157528 µ$
//! - subagent, claude-haiku-4-5-20251001 → claude-haiku-4-5 ($1 in, $5 out,
//!   $2 1h cache write, $0.10 cache read):
//!   6×1 + 300×5 + 4300×2 + 4000×0.10
//!   = 6 + 1500 + 8600 + 400 = 10506 µ$
//! - session total: 157528 + 10506 = 168034 µ$ = $0.168034
//!
//! The turn-2 messages attributed to skill `code-review` (opus-5-5):
//!   3×4 + 650×20 + 2100×8 + 43300×0.20
//!   = 12 + 13000 + 16800 + 8660 = 38472 µ$

mod common;

use chrono::NaiveDate;
use claudit::pricing::{PriceTable, Usd};
use claudit::stats::Filter;
use claudit::stats::consumption::TokenTotals;
use claudit::stats::cost;
use common::{TestEnv, fixtures_dir};

const ACME: &str = "-Users-alice-code-acme-api";
const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";
const SESSION_B: &str = "2b7e4f10-3c5d-4e6f-8a9b-1c2d3e4f5a6b";
const UNKNOWN_SESSION: &str = "7e3a9c51-4d2b-4f8e-9a1c-3b5d7f9e1a2c";
const SPLIT_SESSION: &str = "4f6b8d02-5e7a-4c1b-9d3f-6a8c0e2b4d61";

fn drop_session_a(env: &TestEnv) {
    env.drop_transcript_fixture(&format!("{ACME}/{SESSION_A}.jsonl"));
    env.drop_transcript_fixture(&format!(
        "{ACME}/{SESSION_A}/subagents/agent-a1f3c5e7b9d2c4e6f.jsonl"
    ));
}

/// Drops a cost fixture session, kept outside `projects/` so the other test
/// files' whole-tree fixtures (and their expected totals) are unchanged.
fn drop_cost_fixture(env: &TestEnv, session: &str) {
    let path = fixtures_dir().join(format!("transcripts/cost/{session}.jsonl"));
    let contents = std::fs::read_to_string(path).expect("read cost fixture");
    env.drop_transcript(&format!("{ACME}/{session}.jsonl"), &contents);
}

fn drop_unknown_model_session(env: &TestEnv) {
    drop_cost_fixture(env, UNKNOWN_SESSION);
}

fn builtin() -> &'static PriceTable {
    PriceTable::builtin()
}

#[test]
fn a_session_costs_its_tokens_at_list_prices() {
    let env = TestEnv::new();
    drop_session_a(&env);
    env.ingest();

    let total = cost::total_cost(&env.db(), &Filter::default(), builtin()).unwrap();
    assert_eq!(total.cost.total(), Some(Usd::from_micros(168_034)));
    assert!(total.cost.is_complete());

    let sessions = cost::cost_by_session(&env.db(), &Filter::default(), builtin()).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].key, SESSION_A);
    assert_eq!(sessions[0].cost.total(), Some(Usd::from_micros(168_034)));
}

#[test]
fn a_price_correction_changes_the_cost_without_reingesting() {
    let env = TestEnv::new();
    drop_session_a(&env);
    env.ingest();
    let before = cost::total_cost(&env.db(), &Filter::default(), builtin()).unwrap();

    // Opus 5.5 at double its list prices, Haiku 4.5 unchanged.
    let corrected = PriceTable::from_toml(
        r#"
        version = "test"
        source = "tests/cost.rs"

        [models.claude-opus-5-5]
        input = 8.0
        output = 40.0
        cache_write_5m = 10.0
        cache_write_1h = 16.0
        cache_read = 0.40

        [models.claude-haiku-4-5]
        input = 1.0
        output = 5.0
        cache_write_5m = 1.25
        cache_write_1h = 2.0
        cache_read = 0.10
        "#,
    )
    .unwrap();
    let after = cost::total_cost(&env.db(), &Filter::default(), &corrected).unwrap();

    assert_eq!(before.cost.total(), Some(Usd::from_micros(168_034)));
    // 2 × 157528 + 10506
    assert_eq!(after.cost.total(), Some(Usd::from_micros(325_562)));
    assert_eq!(after.tokens, before.tokens);
}

#[test]
fn an_unknown_model_has_its_tokens_but_an_unknown_cost() {
    let env = TestEnv::new();
    drop_unknown_model_session(&env);
    env.ingest();

    let total = cost::total_cost(&env.db(), &Filter::default(), builtin()).unwrap();
    assert_eq!(
        total.tokens,
        TokenTotals {
            input: 10,
            output: 400,
            cache_write: 2000,
            cache_read: 6000,
        }
    );
    assert_eq!(total.cost.total(), None);
    assert!(!total.cost.is_complete());
    assert_eq!(
        total.cost.unknown_models,
        vec!["claude-nebula-9".to_owned()]
    );

    let models = cost::cost_by_model(&env.db(), &Filter::default(), builtin()).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].key, "claude-nebula-9");
    assert_eq!(models[0].cost.total(), None);
}

#[test]
fn a_mix_of_known_and_unknown_models_reports_the_known_part_as_incomplete() {
    let env = TestEnv::new();
    drop_session_a(&env);
    drop_unknown_model_session(&env);
    env.ingest();

    let total = cost::total_cost(&env.db(), &Filter::default(), builtin()).unwrap();
    assert_eq!(total.cost.known, Usd::from_micros(168_034));
    assert!(!total.cost.is_complete());
    assert_eq!(total.cost.total(), None);

    let sessions = cost::cost_by_session(&env.db(), &Filter::default(), builtin()).unwrap();
    let a = sessions.iter().find(|s| s.key == SESSION_A).unwrap();
    let unknown = sessions.iter().find(|s| s.key == UNKNOWN_SESSION).unwrap();
    assert_eq!(a.cost.total(), Some(Usd::from_micros(168_034)));
    assert_eq!(unknown.cost.total(), None);
}

#[test]
fn cost_is_broken_down_by_model_skill_and_subagent_type() {
    let env = TestEnv::new();
    drop_session_a(&env);
    env.ingest();
    let (db, all) = (env.db(), Filter::default());

    let models = cost::cost_by_model(&db, &all, builtin()).unwrap();
    let models: Vec<_> = models
        .iter()
        .map(|m| (m.key.as_str(), m.cost.total()))
        .collect();
    assert_eq!(
        models,
        vec![
            ("claude-opus-5-5", Some(Usd::from_micros(157_528))),
            ("claude-haiku-4-5-20251001", Some(Usd::from_micros(10_506))),
        ]
    );

    let skills = cost::cost_by_skill(&db, &all, builtin()).unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].key, "code-review");
    assert_eq!(skills[0].cost.total(), Some(Usd::from_micros(38_472)));

    let agents = cost::cost_by_agent_type(&db, &all, builtin()).unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].key, "general-purpose");
    assert_eq!(agents[0].tokens.output, 300);
    assert_eq!(agents[0].cost.total(), Some(Usd::from_micros(10_506)));
}

#[test]
fn the_daily_series_has_tokens_and_cost_per_day_and_model() {
    let env = TestEnv::new();
    drop_session_a(&env);
    env.drop_transcript_fixture(&format!("{ACME}/{SESSION_B}.jsonl"));
    env.ingest();

    let series = cost::daily_series(&env.db(), &Filter::default(), builtin()).unwrap();
    let rows: Vec<_> = series
        .iter()
        .map(|p| (p.day, p.model.as_str(), p.tokens.total(), p.cost.total()))
        .collect();
    // Session B, claude-sonnet-4-6 ($3 in, $15 out, $6 1h write,
    // $0.30 read): 5×3 + 150×15 + 3200×6 + 3000×0.30
    // = 15 + 2250 + 19200 + 900 = 22365 µ$
    let day = |d| NaiveDate::from_ymd_opt(2026, 3, d).unwrap();
    assert_eq!(
        rows,
        vec![
            (
                day(2),
                "claude-haiku-4-5-20251001",
                8_606,
                Some(Usd::from_micros(10_506))
            ),
            (
                day(2),
                "claude-opus-5-5",
                87_469,
                Some(Usd::from_micros(157_528))
            ),
            (
                day(3),
                "claude-sonnet-4-6",
                6_355,
                Some(Usd::from_micros(22_365))
            ),
        ]
    );
}

#[test]
fn model_ids_resolve_through_snapshot_dates_and_context_suffixes() {
    let prices = builtin();
    let base = prices.price("claude-opus-5-5").expect("opus 5.5 priced");
    assert_eq!(prices.price("claude-opus-5-5[1m]"), Some(base));
    assert_eq!(prices.price("claude-opus-5-5-20260801"), Some(base));
    assert!(prices.price("claude-haiku-4-5-20251001").is_some());
    // A newer point release is not silently priced as its family.
    assert_eq!(prices.price("claude-opus-5-7"), None);
    assert_ne!(prices.price("claude-opus-5"), Some(base));
}

#[test]
fn one_hour_cache_writes_are_priced_at_their_own_rate() {
    let env = TestEnv::new();
    drop_cost_fixture(&env, SPLIT_SESSION);
    env.ingest();

    // One claude-opus-5-5 response streamed over two entries: input 10,
    // output 100 (MAX of 40 and 100), cache read 2000, and 4000 cache-write
    // tokens split 1000 5-minute ($5/MTok) + 3000 1-hour ($8/MTok):
    //   10×4 + 100×20 + 1000×5 + 3000×8 + 2000×0.20
    //   = 40 + 2000 + 5000 + 24000 + 400 = 31440 µ$
    // (all 4000 at the 5m rate would be 40 + 2000 + 20000 + 400 = 22440 µ$).
    let total = cost::total_cost(&env.db(), &Filter::default(), builtin()).unwrap();
    assert_eq!(
        total.tokens,
        TokenTotals {
            input: 10,
            output: 100,
            cache_write: 4000,
            cache_read: 2000,
        }
    );
    assert_eq!(total.cost.total(), Some(Usd::from_micros(31_440)));
}
