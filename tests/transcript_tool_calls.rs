//! Tool calls read from transcripts, for sessions recorded before
//! `claudit install` (backfilled) or whose hooks are missing.
//!
//! Seam under test: seam 1 only. Transcript fixtures
//! (`tests/fixtures/transcripts/tool_calls`, see its README) are dropped into
//! the Claude projects dir, hook fixtures go in through the hook entry point,
//! `TestEnv::ingest` loads both, and assertions are made on the typed
//! reports: `stats::tools` rankings (durations, failures, and how many
//! durations are transcript estimates), `stats::time` (turn decomposition,
//! waiting) and `stats::subagents` (calls per run).

mod common;

use chrono::{DateTime, Duration, TimeZone, Utc};
use claudit::stats::Filter;
use claudit::stats::time::TimeSplit;
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

fn split_ms(model: i64, tool: i64, waiting: i64, subagent: i64) -> TimeSplit {
    TimeSplit {
        model: Duration::milliseconds(model),
        tool: Duration::milliseconds(tool),
        waiting: Duration::milliseconds(waiting),
        subagent: Duration::milliseconds(subagent),
    }
}

/// Every call of the fixture session is a transcript estimate:
/// tool_result timestamp − tool_use timestamp.
#[test]
fn a_backfilled_session_yields_tool_calls_with_estimated_durations() {
    let env = TestEnv::new();
    env.drop_tool_calls_fixture();
    env.ingest();

    let estimated = |calls, failures, durations: &[u64]| CallStats {
        calls,
        failures,
        total_duration_ms: durations.iter().sum(),
        median_duration_ms: durations.first().copied(),
        p95_duration_ms: durations.last().copied(),
        estimated_duration_calls: calls,
    };
    assert_eq!(
        env.tool_ranking(&Filter::default()),
        vec![
            RankedCalls {
                name: "Bash".into(),
                stats: estimated(2, 1, &[1500, 7000]),
            },
            RankedCalls {
                name: "Agent".into(),
                stats: estimated(1, 0, &[6000]),
            },
            RankedCalls {
                name: "Grep".into(),
                stats: estimated(1, 0, &[400]),
            },
            RankedCalls {
                name: "Read".into(),
                stats: estimated(1, 0, &[200]),
            },
        ]
    );
    assert_eq!(
        env.bash_command_ranking(&Filter::default()),
        vec![
            RankedCalls {
                name: "cargo".into(),
                stats: estimated(1, 1, &[7000]),
            },
            RankedCalls {
                name: "git".into(),
                stats: estimated(1, 0, &[1500]),
            },
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

    // The Grep call ran inside the subagent: it is not main-thread tool
    // time. Main thread, 10:00:00 → 10:00:21: Bash 2 → 3.5 s and 5 → 12 s,
    // Read 13 → 13.2 s (tools, 8.7 s), Agent 14 → 20 s (subagent, 6 s);
    // no hooks, so nothing is waiting.
    let turns = env.turn_times(&Filter::default());
    assert_eq!(turns.len(), 1, "{turns:#?}");
    assert_eq!(
        (turns[0].session_id.as_str(), turns[0].prompt_id.as_str()),
        (SESSION, TURN)
    );
    assert_eq!(
        (turns[0].start, turns[0].end),
        (fixture_at(0, 0), fixture_at(21, 0))
    );
    assert_eq!(turns[0].split, split_ms(6_300, 8_700, 0, 6_000));
    assert!(env.waiting_by_tool(&Filter::default()).is_empty());
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
                total_duration_ms: 1500 + 6400,
                median_duration_ms: Some(1500),
                p95_duration_ms: Some(6400),
                estimated_duration_calls: 1,
            },
        }
    );
    assert_eq!(tools.iter().map(|t| t.stats.calls).sum::<u64>(), 5);
    // Tools: 1.5 + 6.4 + 0.2 s; waiting 5.1 → 5.6 s; subagent 6 s.
    let turns = env.turn_times(&Filter::default());
    assert_eq!(turns[0].split, split_ms(6_400, 8_100, 500, 6_000));
    let waiting = env.waiting_by_tool(&Filter::default());
    assert_eq!(waiting.len(), 1, "{waiting:#?}");
    assert_eq!(
        (waiting[0].tool_name.as_str(), waiting[0].waiting),
        ("Bash", Duration::milliseconds(500))
    );
}

/// A transcript-timed call is tool time from its tool_use to its
/// tool_result, never partly waiting, even when the timestamps are finer
/// than the millisecond its estimated duration is stored in.
#[test]
fn a_transcript_timed_call_never_splits_off_waiting_time() {
    let env = TestEnv::new();
    let session = "0c4e8a26-1d3f-4b5a-9c7e-2f6a8b0d1e3c";
    let entry = |uuid: &str, parent: Option<&str>, timestamp: &str, body: serde_json::Value| {
        let mut entry = json!({
            "parentUuid": parent, "isSidechain": false, "uuid": uuid,
            "timestamp": timestamp, "cwd": "/Users/alice/code/toolbox",
            "sessionId": session, "version": "2.1.284", "gitBranch": "main"
        });
        entry
            .as_object_mut()
            .unwrap()
            .extend(body.as_object().unwrap().clone());
        entry.to_string()
    };
    let assistant = |id: &str, block: serde_json::Value| {
        json!({ "type": "assistant", "message": {
            "model": "claude-sonnet-4-6", "id": id, "role": "assistant", "content": [block],
            "usage": { "input_tokens": 1, "output_tokens": 10,
                       "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0 }
        }})
    };
    let prompt = "a1b2c3d4-0011-4000-8000-000000000011";
    let lines = [
        entry(
            "f1",
            None,
            "2026-03-05T10:00:00.000000Z",
            json!({
                "type": "user", "promptId": prompt,
                "message": { "role": "user", "content": "Read the manifest" }
            }),
        ),
        entry(
            "f2",
            Some("f1"),
            "2026-03-05T10:00:01.000000Z",
            assistant(
                "msg_01Sub1",
                json!({
                    "type": "tool_use", "id": "toolu_01SubMsRead0001", "name": "Read",
                    "input": { "file_path": "/Users/alice/code/toolbox/Cargo.toml" }
                }),
            ),
        ),
        entry(
            "f3",
            Some("f2"),
            "2026-03-05T10:00:02.500400Z",
            json!({
                "type": "user", "promptId": prompt,
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_01SubMsRead0001",
                      "content": "[package]" }
                ]}
            }),
        ),
        entry(
            "f4",
            Some("f3"),
            "2026-03-05T10:00:03.000000Z",
            assistant(
                "msg_01Sub2",
                json!({
                    "type": "text", "text": "Done."
                }),
            ),
        ),
    ];
    env.drop_transcript(
        &format!("-Users-alice-code-toolbox/{session}.jsonl"),
        &format!("{}\n", lines.join("\n")),
    );
    env.ingest();

    let turns = env.turn_times(&Filter::default());
    assert_eq!(
        turns[0].split,
        TimeSplit {
            model: Duration::microseconds(1_499_600),
            tool: Duration::microseconds(1_500_400),
            waiting: Duration::zero(),
            subagent: Duration::zero(),
        }
    );
    assert!(env.waiting_by_tool(&Filter::default()).is_empty());
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
        env.bash_command_ranking(&Filter::default())
            .into_iter()
            .map(|r| (r.name, r.stats.calls, r.stats.total_duration_ms))
            .collect::<Vec<_>>()
    };
    assert_eq!(bash(&env), [("git".to_owned(), 1, 1500)]);

    env.append_transcript(&main, &format!("{}\n", lines[4..].join("\n")));
    env.ingest();

    assert_eq!(
        bash(&env),
        [("cargo".to_owned(), 1, 7000), ("git".to_owned(), 1, 1500)]
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
