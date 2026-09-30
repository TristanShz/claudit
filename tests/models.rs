//! The Models block: usage and API-equivalent cost per model.
//!
//! Seam under test: seam 1 only. The fixture transcripts go in through the
//! Claude projects dir, `TestEnv::ingest` loads them, and assertions are made
//! on the typed `claudit::stats::models::model_usage` report.
//!
//! Hand computation (tokens from `tests/fixtures/transcripts/README.md`,
//! prices from `pricing/prices.toml`, every cache write a 1-hour write,
//! $/MTok = µ$/token; API messages counted once per `message.id`):
//!
//! - claude-opus-5-5, main thread only, sessions `8d0c5a3e` (4 messages:
//!   7 in, 1162 out, 15000 cw, 71300 cr → 157528 µ$) and `5c9d1e22`
//!   (1 message: 2 / 500 / 5000 / 1000 → 8 + 10000 + 40000 + 200 = 50208 µ$):
//!   5 messages, 9 / 1662 / 20000 / 72300 = 93971 tokens, 207736 µ$;
//! - claude-haiku-4-5-20251001, subagent of `8d0c5a3e` only: 2 messages,
//!   6 / 300 / 4300 / 4000 = 8606 tokens, 6 + 1500 + 8600 + 400 = 10506 µ$;
//! - claude-sonnet-4-6, session `2b7e4f10`: 2 messages, 5 / 150 / 3200 /
//!   3000 = 6355 tokens, 15 + 2250 + 19200 + 900 = 22365 µ$.
//!
//! Totals: 108932 tokens, 240607 µ$.

mod common;

use claudit::pricing::{PriceTable, Usd};
use claudit::stats::Filter;
use claudit::stats::consumption::TokenTotals;
use claudit::stats::models::{self, ModelsReport};
use common::TestEnv;

const OPUS: &str = "claude-opus-5-5";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const SONNET: &str = "claude-sonnet-4-6";

fn report(env: &TestEnv, filter: &Filter, prices: &PriceTable) -> ModelsReport {
    models::model_usage(&env.db(), filter, prices).expect("model_usage")
}

fn fixture_archive() -> TestEnv {
    let env = TestEnv::new();
    env.drop_projects_fixture();
    env.ingest();
    env
}

#[test]
fn each_model_reports_its_sessions_messages_tokens_and_cost() {
    let env = fixture_archive();

    let report = report(&env, &Filter::default(), PriceTable::builtin());

    let names: Vec<&str> = report.models.iter().map(|m| m.model.as_str()).collect();
    assert_eq!(names, [OPUS, SONNET, HAIKU], "most expensive first");

    let opus = &report.models[0];
    assert_eq!(opus.sessions, 2);
    assert_eq!(opus.api_messages, 5);
    assert_eq!(
        opus.tokens,
        TokenTotals {
            input: 9,
            output: 1662,
            cache_write: 20000,
            cache_read: 72300
        }
    );
    assert_eq!(opus.cost.total(), Some(Usd::from_micros(207_736)));
    assert_eq!(opus.main.api_messages, 5);
    assert_eq!(opus.subagents.api_messages, 0);
    let share = opus.tokens.cache_read_share().unwrap();
    assert!((share - 72300.0 / 92309.0).abs() < 1e-12, "{share}");

    let haiku = &report.models[2];
    assert_eq!(haiku.sessions, 1);
    assert_eq!(haiku.api_messages, 2);
    assert_eq!(haiku.tokens.total(), 8606);
    assert_eq!(haiku.main.api_messages, 0);
    assert_eq!(haiku.subagents.api_messages, 2);
    assert_eq!(haiku.subagents.tokens.total(), 8606);
    assert_eq!(haiku.subagents.cost.total(), Some(Usd::from_micros(10_506)));

    let sonnet = &report.models[1];
    assert_eq!(sonnet.sessions, 1);
    assert_eq!(sonnet.api_messages, 2);
    assert_eq!(sonnet.tokens.total(), 6355);
    assert_eq!(sonnet.cost.total(), Some(Usd::from_micros(22_365)));

    assert_eq!(report.total_tokens.total(), 108_932);
    assert_eq!(report.total_cost.total(), Some(Usd::from_micros(240_607)));
}

#[test]
fn shares_are_of_the_filtered_totals() {
    let env = fixture_archive();

    let report = report(&env, &Filter::default(), PriceTable::builtin());

    let opus = &report.models[0];
    let token_share = report.token_share(opus);
    assert!((token_share - 93971.0 / 108932.0).abs() < 1e-12);
    let cost_share = report.cost_share(opus).unwrap();
    assert!((cost_share - 207736.0 / 240607.0).abs() < 1e-12);
    let sum: f64 = report.models.iter().map(|m| report.token_share(m)).sum();
    assert!((sum - 1.0).abs() < 1e-12);
}

#[test]
fn a_model_missing_from_the_price_table_has_an_unknown_cost() {
    let env = fixture_archive();
    // A table that only prices Opus 5.5.
    let prices = PriceTable::from_toml(
        r#"
        version = "test"
        source = "tests/models.rs"

        [models.claude-opus-5-5]
        input = 4.0
        output = 20.0
        cache_write_5m = 5.0
        cache_write_1h = 8.0
        cache_read = 0.20
        "#,
    )
    .unwrap();

    let report = report(&env, &Filter::default(), &prices);

    let sonnet = report.models.iter().find(|m| m.model == SONNET).unwrap();
    assert_eq!(sonnet.cost.total(), None);
    assert_eq!(report.cost_share(sonnet), None);
    let opus = report.models.iter().find(|m| m.model == OPUS).unwrap();
    assert_eq!(opus.cost.total(), Some(Usd::from_micros(207_736)));
    // Opus is all of the priced cost; the whole is incomplete.
    assert_eq!(report.cost_share(opus), Some(1.0));
    assert_eq!(report.total_cost.total(), None);
}

#[test]
fn a_model_filter_keeps_only_that_model() {
    let env = fixture_archive();

    let filter = Filter {
        model: Some(HAIKU.to_owned()),
        ..Filter::default()
    };
    let report = report(&env, &filter, PriceTable::builtin());

    assert_eq!(report.models.len(), 1);
    assert_eq!(report.models[0].model, HAIKU);
    assert_eq!(report.total_tokens.total(), 8606);
}
