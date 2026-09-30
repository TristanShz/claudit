//! Session detail (#12): one session's header, totals, turn timeline
//! segments, tools, skills and subagents.
//!
//! Seam under test: seam 1 (hook payloads and transcripts in,
//! `claudit::stats::session_detail::session_detail` out).
//!
//! Expected positions come from the receive times in
//! `tests/fixtures/hooks/README.md`, plus one Read call running in
//! parallel with turn 1's Bash call: PreToolUse at 18.0 s, PostToolUse at
//! 22.0 s with `duration_ms` 4000 (it ran 18.0 → 22.0 s, overlapping the
//! Bash call's 15.9 → 19.9 s).

mod common;

use chrono::Duration;
use claudit::pricing::PriceTable;
use claudit::stats::session_detail::{SessionDetail, session_detail};
use claudit::stats::time::SegmentKind::{self, Model, Subagent, Tool, Waiting};
use common::{TestEnv, t0};
use serde_json::json;

const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";

fn at(ms: i64) -> chrono::DateTime<chrono::Utc> {
    t0() + Duration::milliseconds(ms)
}

/// Session A with a Read call in parallel with turn 1's Bash call.
fn session_a_with_parallel_tools() -> TestEnv {
    let env = TestEnv::new();
    env.drop_projects_fixture();
    env.replay_session_a_hooks();
    let parallel = |p: &mut serde_json::Value| {
        p["session_id"] = json!(SESSION_A);
        p["prompt_id"] = json!("a1b2c3d4-0001-4000-8000-000000000001");
        p["tool_name"] = json!("Read");
        p["tool_use_id"] = json!("toolu_01AcmeParallelRead");
        p["tool_input"] = json!({ "file_path": "/Users/alice/code/acme-api/Cargo.toml" });
    };
    env.at(at(18_000))
        .hook_fixture_with("pre_tool_use_bash.json", parallel);
    env.at(at(22_000))
        .hook_fixture_with("post_tool_use_read.json", |p| {
            parallel(p);
            p["duration_ms"] = json!(4000);
        });
    env.ingest();
    env
}

fn detail(env: &TestEnv, session_id: &str) -> Option<SessionDetail> {
    session_detail(&env.db(), session_id, PriceTable::builtin()).expect("session_detail")
}

/// A turn's segments as (kind, start ms, end ms) offsets from `t0()`.
fn segments(detail: &SessionDetail, turn: usize) -> Vec<(SegmentKind, i64, i64)> {
    detail.turns[turn]
        .segments
        .iter()
        .map(|s| {
            (
                s.kind,
                (s.start - t0()).num_milliseconds(),
                (s.end - t0()).num_milliseconds(),
            )
        })
        .collect()
}

#[test]
fn turn_segments_are_positioned_and_sized_in_time() {
    let env = session_a_with_parallel_tools();

    let detail = detail(&env, SESSION_A).expect("session A is known");

    assert_eq!(detail.turns.len(), 2);
    // Turn 1: the model until PreToolUse Bash, the permission prompt until
    // Bash starts, Bash and the parallel Read as one tool stretch, then the
    // model until Stop.
    assert_eq!(
        segments(&detail, 0),
        [
            (Model, 0, 4_100),
            (Waiting, 4_100, 15_900),
            (Tool, 15_900, 22_000),
            (Model, 22_000, 30_300),
        ]
    );
    // Turn 2: the model, the Agent call's 0.1 s before starting, the
    // subagent run, the model until Stop.
    assert_eq!(
        segments(&detail, 1),
        [
            (Model, 300_000, 302_100),
            (Waiting, 302_100, 302_200),
            (Subagent, 302_200, 365_600),
            (Model, 365_600, 380_300),
        ]
    );
    assert_eq!(
        detail.turns[0].prompt_text.as_deref(),
        Some("Run the test suite and tell me what fails")
    );
}

#[test]
fn the_session_totals_sum_its_turns_and_messages() {
    let env = session_a_with_parallel_tools();

    let detail = detail(&env, SESSION_A).unwrap();

    assert_eq!(detail.cwd.as_deref(), Some("/Users/alice/code/acme-api"));
    assert_eq!(detail.git_branch.as_deref(), Some("main"));
    assert_eq!(detail.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        detail.first_prompt.as_deref(),
        Some("Run the test suite and tell me what fails")
    );
    // Main thread 7 + 1162 + 15000 + 71300, subagent 6 + 300 + 4300 + 4000.
    assert_eq!(detail.tokens.total(), 96_075);
    assert!(detail.cost.is_complete() && detail.cost.known.picos() > 0);
    // Turn 1: model 4.1 + 8.3 s, tools 6.1 s, waiting 11.8 s; turn 2:
    // model 16.8 s, waiting 0.1 s, subagent 63.4 s.
    assert_eq!(detail.time.model, Duration::milliseconds(29_200));
    assert_eq!(detail.time.tool, Duration::milliseconds(6_100));
    assert_eq!(detail.time.waiting, Duration::milliseconds(11_900));
    assert_eq!(detail.time.subagent, Duration::milliseconds(63_400));
    // Its first transcript entry.
    assert_eq!(detail.started_at, Some(t0()));
}

#[test]
fn the_detail_lists_the_sessions_tools_skills_and_subagents() {
    let env = session_a_with_parallel_tools();

    let detail = detail(&env, SESSION_A).unwrap();

    // The parallel Read and the subagent's Read, the Agent call, Bash.
    let tools: Vec<(&str, u64)> = detail
        .tools
        .iter()
        .map(|t| (t.name.as_str(), t.stats.calls))
        .collect();
    assert_eq!(tools, [("Read", 2), ("Agent", 1), ("Bash", 1)]);

    let skills: Vec<&str> = detail.skills.iter().map(|s| s.skill.as_str()).collect();
    assert_eq!(skills, ["code-review"]);

    assert_eq!(detail.subagents.len(), 1);
    let run = &detail.subagents[0];
    assert_eq!(run.agent_type, "general-purpose");
    assert_eq!(run.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    assert_eq!(
        run.prompt_id.as_deref(),
        Some("a1b2c3d4-0002-4000-8000-000000000002")
    );
}

#[test]
fn an_unknown_session_has_no_detail() {
    let env = session_a_with_parallel_tools();

    assert_eq!(detail(&env, "00000000-0000-0000-0000-000000000000"), None);
}
