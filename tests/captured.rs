//! Payloads and transcripts captured from a real Claude Code session
//! (`tests/fixtures/*/captured-2.1.284/`, anonymized): one headless turn
//! that runs Bash, reads a file, runs a failing command and launches a
//! background general-purpose subagent, then a second turn Claude Code opens
//! for the subagent's completion notice.
//!
//! Seam under test: seam 1 (library end to end). The captured hooks go in
//! through the hook entry point at their recorded receive times, the
//! captured transcripts are dropped into the projects dir, ingest runs, and
//! assertions are made on the typed stats reports.

mod common;

use chrono::Duration;
use claudit::stats::Filter;
use common::TestEnv;

const VERSION: &str = "2.1.284";
const SESSION: &str = "8b7fcd89-94d5-4cb5-8ee2-da8805b15c80";

fn captured_session() -> TestEnv {
    let env = TestEnv::new();
    env.replay_captured_hooks(VERSION);
    env.drop_captured_transcripts(VERSION);
    env.ingest();
    env
}

#[test]
fn every_captured_payload_is_archived_and_projected() {
    let env = TestEnv::new();
    env.replay_captured_hooks(VERSION);

    let report = env.ingest_report();

    assert_eq!(report.events, 18);
    assert_eq!(report.unprojected_events, 0, "{:?}", env.log());
    assert_eq!(report.skipped_lines, 0);
}

#[test]
fn the_captured_tools_are_ranked_with_the_failure() {
    let env = captured_session();

    let ranking = env.tool_ranking(&Filter::default());

    let calls = |name: &str| {
        ranking
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} missing from {ranking:?}"))
            .stats
            .clone()
    };
    // Two main-thread calls (one failed) and one inside the subagent.
    assert_eq!(calls("Bash").calls, 3);
    assert_eq!(calls("Bash").failures, 1);
    assert_eq!(calls("Read").calls, 1);
    assert_eq!(calls("Agent").calls, 1);
    assert_eq!(ranking.len(), 3, "{ranking:?}");
}

#[test]
fn every_captured_turn_splits_its_wall_time_exactly() {
    let env = captured_session();

    let turns = env.turn_times(&Filter::default());

    assert_eq!(turns.len(), 2, "{turns:?}");
    for turn in &turns {
        assert_eq!(turn.session_id, SESSION);
        assert!(turn.end > turn.start, "{turn:?}");
        assert_eq!(
            turn.split.wall() - turn.split.background,
            turn.end - turn.start,
            "{turn:?}"
        );
        let tiled: Duration = turn
            .segments
            .iter()
            .fold(Duration::zero(), |sum, s| sum + (s.end - s.start));
        assert_eq!(tiled, turn.split.wall(), "{turn:?}");
    }
    // The first turn is bounded by its UserPromptSubmit and Stop hooks.
    let first = turns
        .iter()
        .find(|t| t.prompt_id == "662d5648-37e0-4239-affd-4ea405085399")
        .expect("first turn");
    assert_eq!(first.end - first.start, Duration::microseconds(10_683_313));
    assert!(first.split.tool > Duration::zero(), "{first:?}");
    // Its subagent, launched in the background, outlives the Stop hook.
    assert_eq!(first.split.background, Duration::microseconds(189_068));
}

#[test]
fn the_captured_subagent_run_has_its_type_model_and_tokens() {
    let env = captured_session();

    let runs = env.subagent_runs(&Filter::default());

    assert_eq!(runs.len(), 1, "{runs:?}");
    let run = &runs[0];
    assert_eq!(run.agent_id, "ab3adec90281dd9a2");
    assert_eq!(run.agent_type, "general-purpose");
    assert_eq!(
        run.parent_tool_use_id.as_deref(),
        Some("toolu_01NyHWzYp98PsmVpfgfxdmay")
    );
    assert_eq!(run.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    assert_eq!(run.tool_calls, 1);
    assert!(run.tokens.output > 0, "{run:?}");
    assert!(run.tokens.total() > 0, "{run:?}");
}
