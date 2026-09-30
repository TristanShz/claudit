//! Time decomposition: model / tools / waiting / subagents (#8).
//!
//! Seams under test (seam 1 of `tests/common`): hook fixtures replayed
//! through `claudit::hook::run` with controlled receive times, fixture
//! transcripts dropped in place, `claudit::ingest::run`, then assertions on
//! the typed reports `stats::time::{time_breakdown, turn_times}` only.

mod common;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use claudit::stats::Filter;
use claudit::stats::time::{SegmentKind, TimeSplit, TurnTime};
use common::{TestEnv, t0};
use serde_json::json;

const ACME: &str = "-Users-alice-code-acme-api";
const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";
const TURN_A1: &str = "a1b2c3d4-0001-4000-8000-000000000001";
const TURN_A2: &str = "a1b2c3d4-0002-4000-8000-000000000002";

fn ms(ms: i64) -> Duration {
    Duration::milliseconds(ms)
}

fn at(offset_ms: i64) -> DateTime<Utc> {
    t0() + ms(offset_ms)
}

fn split(model: i64, tool: i64, waiting: i64, subagent: i64) -> TimeSplit {
    TimeSplit {
        model: ms(model),
        tool: ms(tool),
        waiting: ms(waiting),
        subagent: ms(subagent),
    }
}

/// Session A: hooks plus its main and subagent transcripts.
fn session_a() -> TestEnv {
    let env = TestEnv::new();
    env.drop_transcript_fixture(&format!("{ACME}/{SESSION_A}.jsonl"));
    env.drop_transcript_fixture(&format!(
        "{ACME}/{SESSION_A}/subagents/agent-a1f3c5e7b9d2c4e6f.jsonl"
    ));
    env.replay_session_a_hooks();
    env.ingest();
    env
}

fn turn<'a>(turns: &'a [TurnTime], prompt_id: &str) -> &'a TurnTime {
    turns
        .iter()
        .find(|t| t.prompt_id == prompt_id)
        .unwrap_or_else(|| panic!("turn {prompt_id} missing from {turns:#?}"))
}

/// A hook-only turn of session `s-par` (no transcript).
fn hook(env: &TestEnv, at_ms: i64, payload: serde_json::Value) {
    let mut payload = payload;
    payload["session_id"] = json!("s-par");
    payload["prompt_id"] = json!("p-par");
    payload["cwd"] = json!("/Users/alice/code/acme-api");
    env.at(at(at_ms)).hook(&payload);
}

fn submit(env: &TestEnv, at_ms: i64) {
    hook(
        env,
        at_ms,
        json!({"hook_event_name": "UserPromptSubmit", "prompt": "read both files"}),
    );
}

fn stop(env: &TestEnv, at_ms: i64) {
    hook(env, at_ms, json!({"hook_event_name": "Stop"}));
}

fn pre(env: &TestEnv, at_ms: i64, id: &str, tool: &str) {
    hook(
        env,
        at_ms,
        json!({"hook_event_name": "PreToolUse", "tool_name": tool, "tool_use_id": id, "tool_input": {}}),
    );
}

fn post(env: &TestEnv, at_ms: i64, id: &str, tool: &str, duration_ms: i64) {
    hook(
        env,
        at_ms,
        json!({"hook_event_name": "PostToolUse", "tool_name": tool, "tool_use_id": id,
               "tool_input": {}, "duration_ms": duration_ms}),
    );
}

#[test]
fn a_permission_prompt_gap_counts_as_waiting_not_tool_time() {
    let env = session_a();

    let turns = env.turn_times(&Filter::default());
    let t1 = turn(&turns, TURN_A1);

    // Submit 0 s, Bash PreToolUse 4.1 s, permission granted and Bash runs
    // 15.9 s → 19.9 s (duration 4 s), Stop 30.3 s.
    assert_eq!(t1.split, split(14_500, 4_000, 11_800, 0));
    assert_eq!(t1.split.wall(), ms(30_300));
    assert_eq!((t1.start, t1.end), (at(0), at(30_300)));
}

#[test]
fn parallel_tool_calls_overlap_so_tool_time_never_exceeds_wall_time() {
    let env = TestEnv::new();
    submit(&env, 0);
    // Three reads issued together at 1 s, running 1–9 s, 1.5–9.5 s, 2–9.8 s.
    for id in ["t-1", "t-2", "t-3"] {
        pre(&env, 1_000, id, "Read");
    }
    post(&env, 9_000, "t-1", "Read", 8_000);
    post(&env, 9_500, "t-2", "Read", 8_000);
    post(&env, 9_800, "t-3", "Read", 7_800);
    stop(&env, 10_000);
    env.ingest();

    let turns = env.turn_times(&Filter::default());
    let t = turn(&turns, "p-par");
    // Tool time is the union 1–9.8 s, not the 23.8 s sum. The gaps between
    // PreToolUse and the start of t-2 / t-3 are covered by t-1 running.
    assert_eq!(t.split, split(1_200, 8_800, 0, 0));
    assert!(t.split.tool <= t.split.wall());
}

#[test]
fn the_components_sum_to_wall_time_for_every_fixture_turn() {
    let env = session_a();
    // Plus sessions with transcripts only (no hooks, hence no timed turn)
    // and a hook-only turn.
    env.drop_projects_fixture();
    submit(&env, 0);
    pre(&env, 1_000, "t-1", "Bash");
    post(&env, 3_000, "t-1", "Bash", 1_500);
    stop(&env, 4_000);
    env.ingest();

    let turns = env.turn_times(&Filter::default());
    assert_eq!(turns.len(), 3, "{turns:#?}");
    for t in &turns {
        let s = &t.split;
        assert_eq!(
            s.model + s.tool + s.waiting + s.subagent,
            t.end - t.start,
            "{t:#?}"
        );
        // The positioned segments tile the turn exactly, in order.
        let mut cursor = t.start;
        for seg in &t.segments {
            assert_eq!(seg.start, cursor, "{t:#?}");
            assert!(seg.end > seg.start, "{t:#?}");
            cursor = seg.end;
        }
        assert_eq!(cursor, t.end, "{t:#?}");
    }
}

#[test]
fn a_turn_without_hooks_is_counted_but_not_timed() {
    let env = TestEnv::new();
    env.drop_transcript_fixture(&format!("{ACME}/{SESSION_A}.jsonl"));
    env.ingest();

    // Its transcript spans 09:00:00 → 09:00:30, permission prompt
    // included: time comes from hooks only.
    assert!(env.turn_times(&Filter::default()).is_empty());
    assert_eq!(env.time_breakdown(&Filter::default()).turns, 0);
    assert_eq!(env.consumption(&Filter::default()).turns, 2);
}

#[test]
fn a_subagent_run_is_reported_separately_from_main_thread_tools() {
    let env = session_a();

    let turns = env.turn_times(&Filter::default());
    let t2 = turn(&turns, TURN_A2);
    // Agent PreToolUse 302.1 s, runs 302.2 → 365.6 s; its Read call
    // (inside the subagent) is not main-thread tool time.
    assert_eq!(t2.split, split(16_800, 0, 100, 63_400));
    let kinds: Vec<SegmentKind> = t2.segments.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            SegmentKind::Model,
            SegmentKind::Waiting,
            SegmentKind::Subagent,
            SegmentKind::Model
        ]
    );
    assert_eq!(
        (t2.segments[2].start, t2.segments[2].end),
        (at(302_200), at(365_600))
    );
}

#[test]
fn the_breakdown_aggregates_turns_overall_and_per_day() {
    let env = session_a();
    // A second day: one hook-only turn with a 2 s tool call.
    let day2 = 24 * 3_600_000;
    submit(&env, day2);
    pre(&env, day2 + 1_000, "t-1", "Bash");
    post(&env, day2 + 3_000, "t-1", "Bash", 2_000);
    stop(&env, day2 + 5_000);
    env.ingest();

    let breakdown = env.time_breakdown(&Filter::default());
    let day1 = split(14_500 + 16_800, 4_000, 11_800 + 100, 63_400);
    let day2_split = split(3_000, 2_000, 0, 0);
    assert_eq!(breakdown.turns, 3);
    assert_eq!(
        breakdown.total,
        split(14_500 + 16_800 + 3_000, 6_000, 11_900, 63_400)
    );
    let days: Vec<(NaiveDate, TimeSplit)> =
        breakdown.by_day.iter().map(|d| (d.day, d.split)).collect();
    assert_eq!(
        days,
        [
            (NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(), day1),
            (NaiveDate::from_ymd_opt(2026, 3, 3).unwrap(), day2_split),
        ]
    );

    let only_day2 = Filter {
        from: Some(at(day2)),
        ..Filter::default()
    };
    assert_eq!(env.time_breakdown(&only_day2).total, day2_split);
}

#[test]
fn pre_and_post_tool_use_pair_whichever_is_ingested_first() {
    let together = TestEnv::new();
    let post_first = TestEnv::new();
    for env in [&together, &post_first] {
        submit(env, 0);
        stop(env, 10_000);
    }
    pre(&together, 1_000, "t-1", "Bash");
    post(&together, 6_000, "t-1", "Bash", 2_000);
    together.ingest();

    post(&post_first, 6_000, "t-1", "Bash", 2_000);
    post_first.ingest();
    pre(&post_first, 1_000, "t-1", "Bash");
    post_first.ingest();

    let expected = split(5_000, 2_000, 3_000, 0);
    for env in [&together, &post_first] {
        let turns = env.turn_times(&Filter::default());
        assert_eq!(turn(&turns, "p-par").split, expected);
    }
}

#[test]
fn a_failed_call_pairs_with_its_pre_tool_use_like_a_successful_one() {
    let env = TestEnv::new();
    submit(&env, 0);
    pre(&env, 1_000, "t-1", "Bash");
    hook(
        &env,
        6_000,
        json!({"hook_event_name": "PostToolUseFailure", "tool_name": "Bash", "tool_use_id": "t-1",
               "tool_input": {}, "duration_ms": 2_000, "error": "Exit code 1"}),
    );
    stop(&env, 10_000);
    env.ingest();

    let turns = env.turn_times(&Filter::default());
    assert_eq!(turn(&turns, "p-par").split, split(5_000, 2_000, 3_000, 0));
}

#[test]
fn replaying_the_same_hooks_twice_changes_nothing() {
    let env = session_a();
    let before = env.time_breakdown(&Filter::default());

    env.replay_session_a_hooks();
    env.ingest();

    assert_eq!(env.time_breakdown(&Filter::default()), before);
}
