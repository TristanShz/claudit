//! Session trace: readable labels for system-injected prompts, and
//! subagent run time from `SubagentStart` / `SubagentStop`.
//!
//! Seam under test: seam 1 only (hook payloads and transcripts in, typed
//! `claudit::stats` reports out: `stats::subagents`, `stats::time`,
//! `stats::sessions`, `stats::session_detail`).
//!
//! The scenario is the synthetic session `c4e8a2f0-…`
//! (`TestEnv::populate_trace_session`); every offset below is from
//! 2026-03-07 10:00:00 UTC and comes from
//! `tests/fixtures/hooks/README.md`.

mod common;

use chrono::{DateTime, Duration, TimeZone, Utc};
use claudit::stats::Filter;
use common::TestEnv;

const SESSION: &str = "c4e8a2f0-5b3d-4e7a-9f1c-2d6b8e0a4c7e";
const AGENT: &str = "a5d7f9b1c3e5a7b9d";

/// 2026-03-07 10:00:00 UTC plus `ms`.
fn at(ms: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, 7, 10, 0, 0).unwrap() + Duration::milliseconds(ms)
}

fn trace_session() -> TestEnv {
    let env = TestEnv::new();
    env.populate_trace_session();
    env.ingest();
    env
}

#[test]
fn a_subagent_run_lasts_the_sum_of_its_start_to_stop_spans() {
    let env = trace_session();

    let runs = env.subagent_runs(&Filter::default());

    assert_eq!(runs.len(), 1, "{runs:#?}");
    let run = &runs[0];
    assert_eq!(run.agent_id, AGENT);
    // The Agent call returned after 30 ms (a background run); the run was
    // active 14.1 → 40 s, then resumed 60 → 70 s: 25.9 + 10 s.
    assert_eq!(run.duration, Some(Duration::milliseconds(35_900)));
    assert_eq!(
        run.active,
        [(at(14_100), at(40_000)), (at(60_000), at(70_000))]
    );
    assert_eq!(
        run.description.as_deref(),
        Some("Investigate the flaky test")
    );
}

fn ms(ms: i64) -> Duration {
    Duration::milliseconds(ms)
}

#[test]
fn the_main_thread_blocked_on_a_subagent_is_subagent_time_and_a_background_run_is_not() {
    let env = trace_session();

    let turns = env.turn_times(&Filter::default());

    // Only the two turns the hooks timed.
    assert_eq!(turns.len(), 2, "{turns:#?}");
    // Turn 1 (0 → 18 s): the background run (14.1 → 40 s) overlaps the
    // main thread's model time but is not main-thread time; only the
    // Agent call's own 30 ms is.
    let t1 = turns[0].split;
    assert_eq!(t1.model, ms(7_950));
    assert_eq!(t1.waiting, ms(5_720));
    assert_eq!(t1.tool, ms(4_300));
    assert_eq!(t1.subagent, ms(30));
    // Turn 2 (41 → 75 s): TaskOutput blocks on the subagent 56 → 71 s.
    let t2 = turns[1].split;
    assert_eq!(t2.subagent, ms(15_000));
    assert_eq!(t2.tool, ms(50));
    assert_eq!(t2.waiting, ms(0));
    assert_eq!(t2.model, ms(18_950));
    for turn in &turns {
        assert_eq!(turn.split.wall(), turn.end - turn.start);
    }
}

#[test]
fn a_sessions_first_prompt_is_its_first_typed_one_labelled() {
    let env = TestEnv::new();
    env.populate_trace_session();
    // Another session opened by a task notification, then a typed prompt.
    let other = "e7b1c9d3-2a4f-4b6e-8d0c-1f3a5b7d9e2c";
    let prompt = |at_ms: i64, prompt_id: &str, text: &str| {
        env.at(at(at_ms)).hook(&serde_json::json!({
            "session_id": other,
            "prompt_id": prompt_id,
            "cwd": "/Users/alice/code/acme-api",
            "hook_event_name": "UserPromptSubmit",
            "prompt": text,
        }));
    };
    prompt(
        3_600_000,
        "e0000000-0000-4000-8000-000000000001",
        "<task-notification>\n<task-id>bq1</task-id>\n<status>completed</status>\n\
         <summary>Background command \"cargo build &amp;&amp; cargo test\" completed</summary>\n\
         </task-notification>",
    );
    prompt(
        3_700_000,
        "e0000000-0000-4000-8000-000000000002",
        "Now ship it",
    );
    env.at(at(3_710_000)).hook(&serde_json::json!({
        "session_id": other,
        "prompt_id": "e0000000-0000-4000-8000-000000000002",
        "hook_event_name": "Stop",
    }));
    env.ingest();

    let detail = |id: &str| {
        claudit::stats::session_detail::session_detail(
            &env.db(),
            id,
            claudit::pricing::PriceTable::builtin(),
        )
        .unwrap()
        .unwrap()
    };
    assert_eq!(
        detail(SESSION).first_prompt.as_deref(),
        Some("/implement 12")
    );
    assert_eq!(detail(other).first_prompt.as_deref(), Some("Now ship it"));
    let sessions = env.sessions(&Filter::default());
    let first = |id: &str| {
        sessions
            .iter()
            .find(|s| s.session_id == id)
            .and_then(|s| s.first_prompt.clone())
    };
    assert_eq!(first(SESSION).as_deref(), Some("/implement 12"));
}
