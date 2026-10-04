//! Session trace: when each tool call and subagent ran within a turn, the
//! session's turn list, readable labels for system-injected prompts, and
//! subagent run time from `SubagentStart` / `SubagentStop`.
//!
//! Seam under test: seam 1 only (hook payloads and transcripts in, typed
//! `claudit::stats` reports out: `stats::trace::{session_turns,
//! turn_trace}`, `stats::subagents`, `stats::time`, `stats::sessions`,
//! `stats::session_detail`).
//!
//! The scenario is the synthetic session `c4e8a2f0-…`
//! (`TestEnv::populate_trace_session`); every offset below is from
//! 2026-03-07 10:00:00 UTC and comes from
//! `tests/fixtures/hooks/README.md`.

mod common;

use chrono::{DateTime, Duration, TimeZone, Utc};
use claudit::stats::Filter;
use claudit::stats::prompt::PromptKind;
use claudit::stats::trace::LaneKind;
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
    // Between the turns the run goes on alone 18 → 40 s, then pauses until
    // 60 s: background time counts its active spans only.
    assert_eq!(t1.background, ms(22_000));
    assert_eq!(t2.background, ms(0));
    for turn in &turns {
        assert_eq!(
            turn.split.wall() - turn.split.background,
            turn.end - turn.start
        );
    }
}

fn turns(env: &TestEnv) -> Vec<claudit::stats::trace::TurnRow> {
    claudit::stats::trace::session_turns(
        &env.db(),
        SESSION,
        claudit::activities::ActivityRules::builtin(),
        claudit::pricing::PriceTable::builtin(),
    )
    .expect("session_turns")
}

/// A turn as (number, start, prompt kind, label, duration).
type TurnLine<'a> = (
    usize,
    Option<DateTime<Utc>>,
    PromptKind,
    &'a str,
    Option<Duration>,
);

#[test]
fn every_turn_is_listed_with_a_readable_label_timed_or_not() {
    let env = trace_session();

    let turns = turns(&env);

    let rows: Vec<TurnLine> = turns
        .iter()
        .map(|t| {
            let prompt = t.prompt.as_ref().expect("every turn has a prompt");
            (
                t.number,
                t.started_at,
                prompt.kind,
                prompt.text.as_str(),
                t.duration,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            // Before the hooks: listed, with its transcript start instant
            // but no duration.
            (
                1,
                Some(at(-600_000)),
                PromptKind::Command,
                "/implement 12",
                None
            ),
            (
                2,
                Some(at(0)),
                PromptKind::Typed,
                "Fix the flaky login test",
                Some(ms(18_000))
            ),
            (
                3,
                Some(at(41_000)),
                PromptKind::TaskNotification,
                "Task notification: Agent \"Investigate the flaky test\" finished",
                Some(ms(34_000)),
            ),
            // Injected as a meta entry of the transcript only.
            (
                4,
                Some(at(120_000)),
                PromptKind::SubagentMessage,
                "Message from subagent: Investigate the flaky test",
                None,
            ),
        ]
    );
}

#[test]
fn each_turn_counts_its_calls_subagents_test_runs_and_tokens() {
    let env = trace_session();

    let turns = turns(&env);

    let counts: Vec<(u64, u64, u64, u64, u64, u64)> = turns
        .iter()
        .map(|t| {
            (
                t.tool_calls,
                t.failed_calls,
                t.subagent_runs,
                t.test_runs,
                t.failed_test_runs,
                t.tokens.total(),
            )
        })
        .collect();
    assert_eq!(
        counts,
        [
            // `ls src`, from the transcript only.
            (1, 0, 0, 0, 0, 1_142),
            // Glob (transcript), Bash `cargo test`, Read, Grep, Agent, and
            // the subagent's failed `cargo test login` and WebFetch.
            (7, 1, 1, 2, 1, 2_263),
            // TaskOutput and Edit.
            (2, 0, 0, 0, 0, 0),
            (0, 0, 0, 0, 0, 3_021),
        ]
    );
    assert!(turns[1].cost.is_complete() && turns[1].cost.known.picos() > 0);
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

    let rows = claudit::stats::trace::session_turns(
        &env.db(),
        other,
        claudit::activities::ActivityRules::builtin(),
        claudit::pricing::PriceTable::builtin(),
    )
    .unwrap();
    let labels: Vec<&str> = rows
        .iter()
        .map(|r| r.prompt.as_ref().unwrap().text.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "Task notification: Background command \"cargo build && cargo test\" completed",
            "Now ship it"
        ]
    );
}

const TURN_1: &str = "d0000000-0000-4000-8000-000000000101";
const TURN_2: &str = "d0000000-0000-4000-8000-000000000102";
const TURN_3: &str = "d0000000-0000-4000-8000-000000000103";

fn trace(env: &TestEnv, prompt_id: &str) -> Option<claudit::stats::trace::TurnTrace> {
    claudit::stats::trace::turn_trace(
        &env.db(),
        SESSION,
        prompt_id,
        claudit::activities::ActivityRules::builtin(),
        claudit::pricing::PriceTable::builtin(),
    )
    .expect("turn_trace")
}

/// A call as (lane, tool, activity, summary, launched, execution, wait,
/// success), times in ms from 10:00:00.
type CallRow = (
    usize,
    String,
    String,
    String,
    i64,
    Option<(i64, i64)>,
    Option<i64>,
    Option<bool>,
);

fn call_rows(trace: &claudit::stats::trace::TurnTrace) -> Vec<CallRow> {
    let off = |t: DateTime<Utc>| (t - at(0)).num_milliseconds();
    trace
        .calls
        .iter()
        .map(|c| {
            (
                c.lane,
                c.tool_name.clone(),
                c.activity.clone(),
                c.summary.clone(),
                off(c.launched_at),
                c.exec_start.zip(c.exec_end).map(|(a, b)| (off(a), off(b))),
                c.wait.map(|w| w.num_milliseconds()),
                c.success,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn row(
    lane: usize,
    tool: &str,
    activity: &str,
    summary: &str,
    launched: i64,
    exec: Option<(i64, i64)>,
    wait: Option<i64>,
    success: Option<bool>,
) -> CallRow {
    (
        lane,
        tool.into(),
        activity.into(),
        summary.into(),
        launched,
        exec,
        wait,
        success,
    )
}

#[test]
fn a_turn_trace_positions_every_call_on_its_lane() {
    let env = trace_session();

    let trace = trace(&env, TURN_1).expect("turn 1 is known");

    assert_eq!(trace.turn, turns(&env)[1]);
    assert_eq!(trace.lanes.len(), 2);
    assert_eq!(trace.lanes[0].kind, LaneKind::Main);
    assert_eq!(trace.lanes[0].spans, [(at(0), at(18_000))]);
    assert_eq!(
        trace.lanes[1].kind,
        LaneKind::Subagent {
            agent_id: AGENT.into(),
            agent_type: Some("Explore".into()),
            model: None,
            description: Some("Investigate the flaky test".into()),
        }
    );
    // The background run outlives the turn, and resumes later.
    assert_eq!(
        trace.lanes[1].spans,
        [(at(14_100), at(40_000)), (at(60_000), at(70_000))]
    );
    assert_eq!((trace.start, trace.end), (at(0), at(70_000)));

    assert_eq!(
        call_rows(&trace),
        [
            // Only the transcript saw it: an instant, no length.
            row(
                0,
                "Glob",
                "Search code",
                "**/login*.rs",
                1_000,
                None,
                None,
                Some(true)
            ),
            // A permission prompt 2 → 8 s, then 4 s of execution.
            row(
                0,
                "Bash",
                "Tests",
                "cargo test",
                2_000,
                Some((8_000, 12_000)),
                Some(6_000),
                Some(true)
            ),
            // Read and Grep in parallel.
            row(
                0,
                "Read",
                "Read files",
                "/Users/alice/code/acme-api/src/login.rs",
                3_000,
                Some((3_300, 3_500)),
                Some(300),
                Some(true),
            ),
            row(
                0,
                "Grep",
                "Search code",
                "fn login in /Users/alice/code/acme-api/src",
                3_100,
                Some((3_300, 3_600)),
                Some(200),
                Some(true),
            ),
            row(
                0,
                "Agent",
                "Subagents",
                "Explore · Investigate the flaky test",
                14_000,
                Some((14_020, 14_050)),
                Some(20),
                Some(true),
            ),
            row(
                1,
                "Bash",
                "Tests",
                "cargo test login -- --nocapture",
                20_000,
                Some((20_200, 25_000)),
                Some(200),
                Some(false),
            ),
            row(
                1,
                "WebFetch",
                "Web",
                "https://docs.example.com/flaky-tests",
                30_000,
                Some((30_100, 31_000)),
                Some(100),
                Some(true),
            ),
        ]
    );
    let agent = &trace.calls[4];
    assert_eq!(agent.spawned_lane, Some(1));
    let failed = &trace.calls[5];
    assert_eq!(
        failed.error.as_deref(),
        Some("Exit code 101\ntest login_test ... FAILED")
    );
}

#[test]
fn a_run_of_another_turn_shows_where_it_overlaps_this_one() {
    let env = trace_session();

    let trace = trace(&env, TURN_2).unwrap();

    assert_eq!(trace.lanes.len(), 2);
    // Launched in turn 1, resumed 60 → 70 s while TaskOutput waited on it.
    assert_eq!(trace.lanes[1].spans, [(at(60_000), at(70_000))]);
    assert_eq!(
        call_rows(&trace),
        [
            row(
                0,
                "TaskOutput",
                "Other",
                AGENT,
                56_000,
                Some((56_000, 71_000)),
                None,
                Some(true)
            ),
            row(
                0,
                "Edit",
                "Edit files",
                "/Users/alice/code/acme-api/src/session_store.rs",
                72_000,
                Some((72_000, 72_050)),
                None,
                Some(true),
            ),
        ]
    );
    assert_eq!((trace.start, trace.end), (at(41_000), at(75_000)));
}

#[test]
fn an_untimed_turn_has_a_trace_without_spans() {
    let env = trace_session();

    let untimed = trace(&env, TURN_3).unwrap();

    assert_eq!(untimed.lanes.len(), 1);
    assert!(untimed.lanes[0].spans.is_empty());
    assert!(untimed.calls.is_empty());
    assert_eq!(untimed.start, at(120_000));
    assert_eq!(trace(&env, "no-such-turn"), None);
}

#[test]
fn mcp_and_skill_calls_are_summarized_by_server_tool_and_skill() {
    let env = TestEnv::new();
    let session = "3f2b8c1e-7d4a-4e5b-9c6f-1a2b3c4d5e6f";
    let turn = "b1e2c3d4-5f60-4a7b-8c9d-0e1f2a3b4c5d";
    env.at(at(0)).hook(&serde_json::json!({
        "session_id": session, "prompt_id": turn,
        "hook_event_name": "UserPromptSubmit", "prompt": "Write the release notes",
    }));
    env.hook_fixture_at(at(1_000), "post_tool_use_mcp.json");
    env.hook_fixture_at(at(2_000), "post_tool_use_skill.json");
    env.ingest();

    let trace = claudit::stats::trace::turn_trace(
        &env.db(),
        session,
        turn,
        claudit::activities::ActivityRules::builtin(),
        claudit::pricing::PriceTable::builtin(),
    )
    .unwrap()
    .unwrap();

    let summaries: Vec<(&str, &str, &str)> = trace
        .calls
        .iter()
        .map(|c| {
            (
                c.tool_name.as_str(),
                c.activity.as_str(),
                c.summary.as_str(),
            )
        })
        .collect();
    assert_eq!(
        summaries,
        [
            ("mcp__github__get_issue", "MCP", "github · get_issue"),
            ("Skill", "Skills", "release-notes since v1.2.0"),
        ]
    );
}

#[test]
fn reingest_rebuilds_the_same_turns_traces_and_runs() {
    let env = trace_session();
    let snapshot = |env: &TestEnv| {
        let turns = turns(env);
        let traces: Vec<_> = turns.iter().map(|t| trace(env, &t.prompt_id)).collect();
        (
            turns,
            traces,
            env.subagent_runs(&Filter::default()),
            env.turn_times(&Filter::default()),
        )
    };
    let before = snapshot(&env);
    assert_eq!(before.0.len(), 4);

    env.ingest();
    assert_eq!(snapshot(&env), before, "a second ingest changes nothing");
    claudit::ingest::reingest(&env.paths).expect("reingest succeeds");

    assert_eq!(snapshot(&env), before);
}
