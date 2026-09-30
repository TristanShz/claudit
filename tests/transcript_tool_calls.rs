//! Tool calls read from transcripts, for sessions recorded before
//! `claudit install` (backfilled) or whose hooks are missing.
//!
//! Seam under test: seam 1 only. Transcript fixtures
//! (`tests/fixtures/transcripts/tool_calls`, see its README) are dropped into
//! the Claude projects dir, hook fixtures go in through the hook entry point,
//! `TestEnv::ingest` loads both, and assertions are made on the typed
//! reports: `stats::tools` rankings (calls and failures; durations only
//! from hooks), `stats::time` (no time without hooks), `stats::commands` and
//! `stats::subagents` (calls per run).

mod common;

use chrono::{DateTime, Duration, TimeZone, Utc};
use claudit::stats::Filter;
use claudit::stats::tools::{CallStats, RankedCalls};
use common::TestEnv;
use serde_json::json;

/// Fixture session `6e2d9b47-…` and its one turn.
const SESSION: &str = "6e2d9b47-8c31-4a5f-b0d2-7f4e1a9c3b58";
const TURN: &str = "a1b2c3d4-0010-4000-8000-000000000010";
const SUBAGENT: &str = "a7c9e1b3d5f2a4c6e";

fn fixture_at(seconds: i64, millis: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, 4, 10, 0, 0).unwrap()
        + Duration::seconds(seconds)
        + Duration::milliseconds(millis)
}

/// Every call of the fixture session is counted, with its failure, but
/// none has a duration: no hook timed it, and a transcript's tool_use →
/// tool_result span includes permission prompts.
#[test]
fn a_backfilled_session_yields_tool_calls_without_durations() {
    let env = TestEnv::new();
    env.drop_tool_calls_fixture();
    env.ingest();

    let untimed = |calls, failures| CallStats {
        calls,
        failures,
        ..CallStats::default()
    };
    assert_eq!(
        env.tool_ranking(&Filter::default()),
        vec![
            RankedCalls {
                name: "Bash".into(),
                stats: untimed(2, 1),
            },
            RankedCalls {
                name: "Agent".into(),
                stats: untimed(1, 0),
            },
            RankedCalls {
                name: "Grep".into(),
                stats: untimed(1, 0),
            },
            RankedCalls {
                name: "Read".into(),
                stats: untimed(1, 0),
            },
        ]
    );
    let commands: Vec<(String, CallStats)> = env
        .command_ranking(&Filter::default())
        .commands
        .into_iter()
        .map(|c| (c.command, c.stats))
        .collect();
    assert_eq!(
        commands,
        [
            ("cargo test".to_owned(), untimed(1, 1)),
            ("git status".to_owned(), untimed(1, 0)),
        ]
    );
}

#[test]
fn a_subagent_transcripts_calls_are_attributed_to_the_subagent() {
    let env = TestEnv::new();
    env.drop_tool_calls_fixture();
    env.ingest();

    let runs = env.subagent_runs(&Filter::default());
    assert_eq!(runs.len(), 1, "{runs:#?}");
    assert_eq!(runs[0].agent_id, SUBAGENT);
    assert_eq!(runs[0].tool_calls, 1, "its Grep call");

    // Its turn is not timed (no hooks).
    assert!(env.turn_times(&Filter::default()).is_empty());
}

/// The hooks' view of the fixture's `cargo test` call: announced at
/// 10:00:05.1, failed at 10:00:12 after executing 6.4 s (so 0.5 s waiting
/// on a permission prompt). The transcript has the same call from 10:00:05
/// to 10:00:12.
fn hook_the_cargo_test_call(env: &TestEnv) {
    let call = |p: &mut serde_json::Value| {
        p["session_id"] = json!(SESSION);
        p["prompt_id"] = json!(TURN);
        p["cwd"] = json!("/Users/alice/code/toolbox");
        p["tool_use_id"] = json!("toolu_01ToolboxBash0002");
        p["tool_input"]["command"] = json!("cargo test --all");
    };
    env.at(fixture_at(5, 100))
        .hook_fixture_with("pre_tool_use_bash.json", call);
    env.at(fixture_at(12, 0))
        .hook_fixture_with("post_tool_use_failure_bash.json", |p| {
            call(p);
            p["duration_ms"] = json!(6400);
        });
}

/// What the fixture session reports once the hooks timed `cargo test`.
fn assert_the_hook_timing_wins(env: &TestEnv) {
    let tools = env.tool_ranking(&Filter::default());
    assert_eq!(
        tools[0],
        RankedCalls {
            name: "Bash".into(),
            stats: CallStats {
                calls: 2,
                failures: 1,
                // Only the hook-timed call has a duration.
                total_duration_ms: 6400,
                median_duration_ms: Some(6400),
                p95_duration_ms: Some(6400),
                timed_calls: 1,
            },
        }
    );
    assert_eq!(tools.iter().map(|t| t.stats.calls).sum::<u64>(), 5);
}

#[test]
fn a_result_appended_later_completes_its_call_on_the_next_ingest() {
    let env = TestEnv::new();
    let main = format!("-Users-alice-code-toolbox/{SESSION}.jsonl");
    let text = common::transcripts_fixture_file(&format!("tool_calls/{main}"));
    let lines: Vec<&str> = text.lines().collect();
    // Up to the `cargo test` tool_use (line 4): the session is live and the
    // call still running.
    env.drop_transcript(&main, &format!("{}\n", lines[..4].join("\n")));
    env.ingest();
    let bash = |env: &TestEnv| {
        env.command_ranking(&Filter::default())
            .commands
            .into_iter()
            .map(|c| (c.command, c.stats.calls))
            .collect::<Vec<_>>()
    };
    assert_eq!(bash(&env), [("git status".to_owned(), 1)]);

    env.append_transcript(&main, &format!("{}\n", lines[4..].join("\n")));
    env.ingest();

    assert_eq!(
        bash(&env),
        [("cargo test".to_owned(), 1), ("git status".to_owned(), 1)]
    );
}

#[test]
fn reingest_rebuilds_the_same_calls_whichever_source_came_first() {
    let env = TestEnv::new();
    env.drop_tool_calls_fixture();
    env.ingest();
    hook_the_cargo_test_call(&env);
    env.ingest();
    // Idempotent: nothing new, nothing changes.
    env.ingest();
    assert_the_hook_timing_wins(&env);

    // Reingest replays the hooks first, then re-reads the transcript.
    claudit::ingest::reingest(&env.paths).expect("reingest succeeds");

    assert_the_hook_timing_wins(&env);
    assert_eq!(env.subagent_runs(&Filter::default())[0].tool_calls, 1);
}

#[test]
fn a_call_seen_by_hooks_then_transcript_counts_once_with_the_hook_timing() {
    let env = TestEnv::new();
    hook_the_cargo_test_call(&env);
    env.ingest();
    env.drop_tool_calls_fixture();
    env.ingest();

    assert_the_hook_timing_wins(&env);
}

#[test]
fn a_call_seen_by_transcript_then_hooks_counts_once_with_the_hook_timing() {
    let env = TestEnv::new();
    env.drop_tool_calls_fixture();
    env.ingest();
    hook_the_cargo_test_call(&env);
    env.ingest();

    assert_the_hook_timing_wins(&env);
}
