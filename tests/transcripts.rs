//! Transcript ingestion and backfill (#5).
//!
//! Seams under test (seam 1 of `tests/common`): fixture transcripts are
//! dropped into the Claude projects dir, `claudit::ingest::run` loads them,
//! and assertions are made on the typed reports `stats::consumption`,
//! `stats::sessions` and `stats::ingest_status`, plus the counts in the
//! `IngestReport` that `claudit ingest` prints. Nothing here looks at tables
//! or offsets.

mod common;

use chrono::{Duration, TimeZone, Utc};
use claudit::stats::Filter;
use claudit::stats::consumption::{Consumption, TokenTotals};
use common::TestEnv;

const ACME: &str = "-Users-alice-code-acme-api";
const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";
const SESSION_B: &str = "2b7e4f10-3c5d-4e6f-8a9b-1c2d3e4f5a6b";
const SESSION_C: &str = "5c9d1e22-7a8b-4c9d-8e0f-2a3b4c5d6e7f";

fn main_a() -> String {
    format!("{ACME}/{SESSION_A}.jsonl")
}

fn subagent_a() -> String {
    format!("{ACME}/{SESSION_A}/subagents/agent-a1f3c5e7b9d2c4e6f.jsonl")
}

/// Session A: main thread plus its subagent (see the fixtures README).
fn session_a_tokens() -> TokenTotals {
    TokenTotals {
        input: 13,
        output: 1462,
        cache_write: 19300,
        cache_read: 75300,
    }
}

#[test]
fn a_transcript_and_its_subagent_yield_the_session_turns_and_tokens() {
    let env = TestEnv::new();
    env.drop_transcript_fixture(&main_a());
    env.drop_transcript_fixture(&subagent_a());

    env.ingest();

    assert_eq!(
        env.consumption(&Filter::default()),
        Consumption {
            sessions: 1,
            turns: 2,
            tokens: session_a_tokens(),
        }
    );
    let sessions = env.sessions(&Filter::default());
    assert_eq!(sessions.len(), 1);
    let a = &sessions[0];
    assert_eq!(a.session_id, SESSION_A);
    assert_eq!(a.cwd.as_deref(), Some("/Users/alice/code/acme-api"));
    assert_eq!(a.git_branch.as_deref(), Some("main"));
    assert_eq!(a.version.as_deref(), Some("2.1.284"));
    assert_eq!(
        a.started_at,
        Utc.with_ymd_and_hms(2026, 3, 2, 9, 0, 0).unwrap()
    );
    assert_eq!(
        a.last_activity_at,
        Utc.with_ymd_and_hms(2026, 3, 2, 9, 30, 0).unwrap()
    );
    assert_eq!(a.turns, 2);
    assert_eq!(
        a.first_prompt.as_deref(),
        Some("Run the test suite and tell me what fails")
    );
    assert_eq!(a.tokens, session_a_tokens());
}

#[test]
fn the_cache_read_share_is_the_part_of_input_side_tokens_read_from_cache() {
    let tokens = session_a_tokens();
    // 75300 / (13 + 19300 + 75300)
    let share = tokens.cache_read_share().unwrap();
    assert!((share - 0.795873).abs() < 1e-6, "{share}");
    assert_eq!(TokenTotals::default().cache_read_share(), None);
}

#[test]
fn subagent_tokens_are_counted_under_their_own_model() {
    let env = TestEnv::new();
    env.drop_transcript_fixture(&main_a());
    env.drop_transcript_fixture(&subagent_a());
    env.ingest();

    let haiku = Filter {
        model: Some("claude-haiku-4-5-20251001".into()),
        ..Filter::default()
    };
    assert_eq!(
        env.consumption(&haiku).tokens,
        TokenTotals {
            input: 6,
            output: 300,
            cache_write: 4300,
            cache_read: 4000,
        }
    );
}

#[test]
fn appended_lines_are_processed_once_and_a_partial_line_waits_until_complete() {
    let env = TestEnv::new();
    let full = common::transcript_fixture(&main_a());
    // Turn 1 only: everything before the second prompt.
    let cut = full.find("Have a subagent review").unwrap();
    let cut = full[..cut].rfind('\n').unwrap() + 1;
    let (turn_one, rest) = full.split_at(cut);
    let rest_lines = rest.lines().count() as u64;

    env.drop_transcript(&main_a(), turn_one);
    env.ingest();
    assert_eq!(env.consumption(&Filter::default()).turns, 1);

    // The rest arrives, its last line still being written.
    let (complete, partial) = rest.split_at(rest.len() - 20);
    env.append_transcript(&main_a(), complete);
    let report = env.ingest_report();
    assert_eq!(report.transcript_lines, rest_lines - 1);

    env.append_transcript(&main_a(), partial);
    let report = env.ingest_report();
    assert_eq!(report.transcript_lines, 1);

    let report = env.ingest_report();
    assert_eq!(report.transcript_lines, 0);

    env.drop_transcript_fixture(&subagent_a());
    env.ingest();
    assert_eq!(
        env.consumption(&Filter::default()),
        Consumption {
            sessions: 1,
            turns: 2,
            tokens: session_a_tokens(),
        }
    );
}

#[test]
fn an_unknown_line_shape_is_skipped_counted_and_logged_without_failing() {
    let env = TestEnv::new();
    env.drop_transcript_fixture(&main_a());
    env.append_transcript(&main_a(), "this is not json\n");

    let report = env.ingest_report();

    // The fixture's type-less line plus the garbage line.
    assert_eq!(report.skipped_transcript_lines, 2);
    assert_eq!(env.ingest_status().skipped_transcript_lines, 2);
    assert!(env.log().contains(SESSION_A), "log: {}", env.log());
    assert_eq!(env.consumption(&Filter::default()).turns, 2);

    // Skips are counted once, not again on every run.
    env.ingest();
    assert_eq!(env.ingest_status().skipped_transcript_lines, 2);
}

#[test]
fn a_known_entry_missing_its_required_fields_is_skipped_too() {
    let env = TestEnv::new();
    env.drop_transcript(
        &format!("{ACME}/{SESSION_B}.jsonl"),
        "{\"type\":\"assistant\",\"uuid\":\"u1\",\"message\":\"garbled\"}\n",
    );

    let report = env.ingest_report();

    assert_eq!(report.skipped_transcript_lines, 1);
    assert_eq!(env.consumption(&Filter::default()), Consumption::default());
}

#[test]
fn the_first_run_backfills_every_session_on_disk() {
    let env = TestEnv::new();
    env.drop_projects_fixture();
    assert_eq!(env.ingest_status().transcripts_backfilled_at, None);

    env.ingest();

    let consumption = env.consumption(&Filter::default());
    assert_eq!(consumption.sessions, 3);
    assert_eq!(consumption.turns, 4);
    assert_eq!(
        consumption.tokens,
        TokenTotals {
            input: 13 + 5 + 2,
            output: 1462 + 150 + 500,
            cache_write: 19300 + 3200 + 5000,
            cache_read: 75300 + 3000 + 1000,
        }
    );
    let mut ids: Vec<_> = env
        .sessions(&Filter::default())
        .into_iter()
        .map(|s| s.session_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec![SESSION_B, SESSION_C, SESSION_A]);
    assert!(env.ingest_status().transcripts_backfilled_at.is_some());
}

#[test]
fn ingesting_twice_gives_the_same_results() {
    let env = TestEnv::new();
    env.drop_projects_fixture();

    env.ingest();
    let consumption = env.consumption(&Filter::default());
    let sessions = env.sessions(&Filter::default());
    env.ingest();

    assert_eq!(env.consumption(&Filter::default()), consumption);
    assert_eq!(env.sessions(&Filter::default()), sessions);
}

#[test]
fn entries_copied_into_another_transcript_are_counted_once() {
    // A resumed session starts a new file repeating earlier entries (same
    // uuids, same message ids).
    let env = TestEnv::new();
    env.drop_transcript_fixture(&main_a());
    let copy = common::transcript_fixture(&main_a());
    env.drop_transcript(&format!("{ACME}/0f0f0f0f-resumed.jsonl"), &copy);

    env.ingest();

    let consumption = env.consumption(&Filter::default());
    assert_eq!(consumption.sessions, 1);
    assert_eq!(consumption.turns, 2);
    assert_eq!(consumption.tokens.output, 1162);
}

#[test]
fn consumption_honours_the_branch_project_and_date_filters() {
    let env = TestEnv::new();
    env.drop_projects_fixture();
    env.ingest();

    let login = Filter {
        branch: Some("feat/login".into()),
        ..Filter::default()
    };
    assert_eq!(
        env.consumption(&login),
        Consumption {
            sessions: 1,
            turns: 1,
            tokens: TokenTotals {
                input: 5,
                output: 150,
                cache_write: 3200,
                cache_read: 3000,
            },
        }
    );

    let web_app = Filter {
        project: Some("/Users/alice/code/web-app".into()),
        ..Filter::default()
    };
    assert_eq!(env.consumption(&web_app).sessions, 1);
    assert_eq!(env.consumption(&web_app).tokens.output, 500);

    let march_3 = Filter {
        from: Some(Utc.with_ymd_and_hms(2026, 3, 3, 0, 0, 0).unwrap()),
        to: Some(Utc.with_ymd_and_hms(2026, 3, 3, 0, 0, 0).unwrap() + Duration::days(1)),
        ..Filter::default()
    };
    let consumption = env.consumption(&march_3);
    assert_eq!((consumption.sessions, consumption.turns), (1, 1));
    assert_eq!(consumption.tokens.output, 150);
    assert_eq!(env.sessions(&march_3)[0].session_id, SESSION_B);
}
