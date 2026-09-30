//! Time is measured from hook-recorded data only; consumption covers
//! everything, backfilled sessions included.
//!
//! Sessions imported by the transcript backfill (recorded before
//! `claudit install`: no hook data at all) have tokens, cost, turns and tool
//! calls, but their transcript spans include permission prompts, idle time
//! and background work, so they never contribute time.
//!
//! Seams under test (seam 1 of `tests/common`, nothing else): a mixed
//! archive — fixture session A with its hooks and transcripts, plus the
//! transcript-only session `6e2d9b47-…` (`tests/fixtures/transcripts/
//! tool_calls`) — goes in through the hook entry point and the Claude
//! projects dir, `TestEnv::ingest` loads it, and assertions are made on the
//! typed reports `stats::time` (`time_breakdown`, `turn_times`,
//! `time_coverage`), `stats::tools` rankings, `stats::activities`,
//! `stats::sessions::session_list`, `stats::session_detail`,
//! `stats::skills` and `stats::subagents`.

mod common;

use chrono::{Duration, NaiveDate};
use claudit::stats::Filter;
use claudit::stats::time::TimeSplit;
use claudit::stats::tools::CallStats;
use common::{TestEnv, t0};

const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";
const ACME: &str = "-Users-alice-code-acme-api";
const TURN_A2: &str = "a1b2c3d4-0002-4000-8000-000000000002";
const IMPORTED: &str = "6e2d9b47-8c31-4a5f-b0d2-7f4e1a9c3b58";

fn split(model: i64, tool: i64, waiting: i64, subagent: i64) -> TimeSplit {
    TimeSplit {
        model: Duration::milliseconds(model),
        tool: Duration::milliseconds(tool),
        waiting: Duration::milliseconds(waiting),
        subagent: Duration::milliseconds(subagent),
    }
}

/// Session A (hooks + main and subagent transcripts) and the imported
/// session `6e2d9b47-…`.
fn mixed_archive() -> TestEnv {
    let env = TestEnv::new();
    env.drop_transcript_fixture(&format!("{ACME}/{SESSION_A}.jsonl"));
    env.drop_transcript_fixture(&format!(
        "{ACME}/{SESSION_A}/subagents/agent-a1f3c5e7b9d2c4e6f.jsonl"
    ));
    env.drop_tool_calls_fixture();
    env.replay_session_a_hooks();
    env.ingest();
    env
}

#[test]
fn time_is_measured_only_on_turns_the_hooks_timed() {
    let env = mixed_archive();

    // Session A's two turns (see tests/time_decomposition.rs): turn 1
    // 14.5 s model, 4 s Bash, 11.8 s waiting on a prompt; turn 2 16.8 s
    // model, 0.1 s waiting, 63.4 s subagent. The imported session has no
    // UserPromptSubmit / Stop hooks: no time.
    let breakdown = env.time_breakdown(&Filter::default());
    assert_eq!(breakdown.turns, 2);
    assert_eq!(breakdown.total, split(31_300, 4_000, 11_900, 63_400));
    let days: Vec<NaiveDate> = breakdown.by_day.iter().map(|d| d.day).collect();
    assert_eq!(days, [NaiveDate::from_ymd_opt(2026, 3, 2).unwrap()]);
    let sessions: Vec<String> = env
        .turn_times(&Filter::default())
        .into_iter()
        .map(|t| t.session_id)
        .collect();
    assert_eq!(sessions, [SESSION_A, SESSION_A]);
}

/// Stats of `calls` calls, `failures` failed, of which the hooks timed
/// `timed` (any order).
fn calls(calls: u64, failures: u64, timed: &[u64]) -> CallStats {
    let mut sorted = timed.to_vec();
    sorted.sort_unstable();
    CallStats {
        calls,
        failures,
        total_duration_ms: timed.iter().sum(),
        median_duration_ms: sorted.first().copied(),
        p95_duration_ms: sorted.last().copied(),
        timed_calls: timed.len() as u64,
    }
}

#[test]
fn tool_durations_come_from_hooks_while_counts_include_imported_calls() {
    let env = mixed_archive();

    // Session A (hooks): Bash `cargo test` 4 s, Agent 63.4 s, the
    // subagent's Read 0.7 s. Imported: Bash `git status` and a failed
    // `cargo test`, Read, Agent, and the subagent's Grep, all untimed.
    let ranking: Vec<(String, CallStats)> = env
        .tool_ranking(&Filter::default())
        .into_iter()
        .map(|r| (r.name, r.stats))
        .collect();
    assert_eq!(
        ranking,
        [
            ("Bash".to_owned(), calls(3, 1, &[4_000])),
            ("Agent".to_owned(), calls(2, 0, &[63_400])),
            ("Read".to_owned(), calls(2, 0, &[700])),
            ("Grep".to_owned(), calls(1, 0, &[])),
        ]
    );
    let bash: Vec<(String, CallStats)> = env
        .bash_command_ranking(&Filter::default())
        .into_iter()
        .map(|r| (r.name, r.stats))
        .collect();
    assert_eq!(
        bash,
        [
            ("cargo".to_owned(), calls(2, 1, &[4_000])),
            ("git".to_owned(), calls(1, 0, &[])),
        ]
    );
}

#[test]
fn activity_time_comes_from_hooks_while_calls_include_imported_ones() {
    let env = mixed_archive();

    let breakdown = claudit::stats::activities::activity_breakdown(
        &env.db(),
        &Filter::default(),
        claudit::activities::ActivityRules::builtin(),
    )
    .unwrap();
    let activities: Vec<(String, CallStats)> = breakdown
        .activities
        .iter()
        .map(|a| (a.activity.clone(), a.stats.clone()))
        .collect();
    // Most time first, then most calls: untimed activities come last.
    assert_eq!(
        activities,
        [
            ("Subagents".to_owned(), calls(2, 0, &[63_400])),
            ("Tests".to_owned(), calls(2, 1, &[4_000])),
            ("Read files".to_owned(), calls(1 + 1, 0, &[700])),
            ("Git & GitHub".to_owned(), calls(1, 0, &[])),
            ("Search code".to_owned(), calls(1, 0, &[])),
        ]
    );
    assert_eq!(breakdown.total_duration_ms, 63_400 + 4_000 + 700);
    // The imported calls (2026-03-04) count on their day, with no time.
    let imported_day: Vec<(String, u64, u64)> = breakdown
        .by_day
        .iter()
        .filter(|d| d.day == NaiveDate::from_ymd_opt(2026, 3, 4).unwrap())
        .map(|d| (d.activity.clone(), d.calls, d.duration_ms))
        .collect();
    assert_eq!(
        imported_day,
        [
            ("Git & GitHub".to_owned(), 1, 0),
            ("Read files".to_owned(), 1, 0),
            ("Search code".to_owned(), 1, 0),
            ("Subagents".to_owned(), 1, 0),
            ("Tests".to_owned(), 1, 0),
        ]
    );
}

#[test]
fn a_session_without_any_hook_data_is_flagged_imported_and_has_no_time() {
    let env = mixed_archive();

    let sessions: Vec<(String, bool, TimeSplit, u64, u64)> = env
        .sessions(&Filter::default())
        .into_iter()
        .map(|s| (s.session_id, s.imported, s.time, s.turns, s.tool_calls))
        .collect();
    assert_eq!(
        sessions,
        [
            // 2026-03-04: one turn, five calls, all from its transcripts.
            (IMPORTED.to_owned(), true, TimeSplit::default(), 1, 5),
            (
                SESSION_A.to_owned(),
                false,
                split(31_300, 4_000, 11_900, 63_400),
                2,
                3
            ),
        ]
    );

    let prices = claudit::pricing::PriceTable::builtin();
    let detail = claudit::stats::session_detail::session_detail(&env.db(), IMPORTED, prices)
        .unwrap()
        .unwrap();
    assert!(detail.imported);
    assert_eq!(detail.turn_count, 1, "its turn is counted");
    assert!(detail.turns.is_empty(), "but not timed");
    assert_eq!(detail.time, TimeSplit::default());
    assert!(detail.tokens.total() > 0, "consumption is still reported");
    assert_eq!(detail.tools.iter().map(|t| t.stats.calls).sum::<u64>(), 5);
    assert!(detail.tools.iter().all(|t| t.stats.timed_calls == 0));

    let detail = claudit::stats::session_detail::session_detail(&env.db(), SESSION_A, prices)
        .unwrap()
        .unwrap();
    assert!(!detail.imported);
    assert_eq!(detail.turns.len(), 2);
}

#[test]
fn time_coverage_counts_the_recorded_and_imported_sessions_in_the_filter() {
    let env = mixed_archive();
    let coverage = |filter: &Filter| {
        let c = claudit::stats::time::time_coverage(&env.db(), filter).unwrap();
        (c.recorded_sessions, c.imported_sessions, c.hooks_since)
    };
    // Timed since session A's first UserPromptSubmit, at t0.
    let since = Some(t0());

    assert_eq!(coverage(&Filter::default()), (1, 1, since));

    // Filters are honoured: the imported session's project only.
    let toolbox = Filter {
        project: Some("/Users/alice/code/toolbox".to_owned()),
        ..Filter::default()
    };
    assert_eq!(coverage(&toolbox), (0, 1, since));
    assert_eq!(env.time_breakdown(&toolbox).turns, 0);
    assert_eq!(env.tool_ranking(&toolbox).len(), 4, "its calls still count");
    let acme = Filter {
        project: Some("/Users/alice/code/acme-api".to_owned()),
        ..Filter::default()
    };
    assert_eq!(coverage(&acme), (1, 0, since));
}

#[test]
fn a_subagent_run_has_a_duration_only_when_the_hooks_timed_it() {
    let env = mixed_archive();

    let runs: Vec<(String, Option<Duration>, u64)> = env
        .subagent_runs(&Filter::default())
        .into_iter()
        .map(|r| (r.session_id, r.duration, r.tool_calls))
        .collect();
    assert_eq!(
        runs,
        [
            // SubagentStart 302.2 s → SubagentStop 365.5 s.
            (
                SESSION_A.to_owned(),
                Some(Duration::milliseconds(63_300)),
                1
            ),
            // Its transcript has a first and a last entry, but no hook
            // timed it.
            (IMPORTED.to_owned(), None, 1),
        ]
    );
}

#[test]
fn a_skill_gets_no_time_in_a_turn_whose_stop_the_hooks_missed() {
    let env = TestEnv::new();
    env.drop_transcript_fixture(&format!("{ACME}/{SESSION_A}.jsonl"));
    // Turn 2 (the typed /code-review) never got its Stop hook.
    env.replay_session_a_hooks_with(|_, p| {
        if p["hook_event_name"] == "Stop" && p["prompt_id"] == TURN_A2 {
            p["hook_event_name"] = serde_json::json!("Notification");
        }
    });
    env.ingest();

    let skills = env.skills(&Filter::default());
    let review = skills.iter().find(|s| s.skill == "code-review").unwrap();
    assert_eq!(review.user_invocations, 1);
    assert_eq!(review.attributed_time, Duration::zero());
}
