//! The overview page's archive-wide facts (#11): the filter bar's choices,
//! the time of the last ingest and whether there is any data at all.
//!
//! Seam under test: seam 1 (hook payloads and transcripts in, typed
//! `claudit::stats` reports out).

mod common;

use chrono::Duration;
use claudit::stats;
use common::{TestEnv, t0};

#[test]
fn filter_options_list_the_distinct_projects_branches_and_models() {
    let env = TestEnv::new();
    env.drop_projects_fixture();
    env.ingest();

    let options = stats::filter_options::filter_options(&env.db()).unwrap();

    assert_eq!(
        options.projects,
        ["/Users/alice/code/acme-api", "/Users/alice/code/web-app"]
    );
    assert_eq!(options.branches, ["feat/login", "main"]);
    assert_eq!(
        options.models,
        [
            "claude-haiku-4-5-20251001",
            "claude-opus-5-5",
            "claude-sonnet-4-6"
        ]
    );
}

#[test]
fn filter_options_of_an_empty_archive_are_empty() {
    let env = TestEnv::new();
    env.ingest();

    let options = stats::filter_options::filter_options(&env.db()).unwrap();

    assert!(options.projects.is_empty());
    assert!(options.branches.is_empty());
    assert!(options.models.is_empty());
}

#[test]
fn the_ingest_status_tells_when_the_last_catch_up_ran() {
    let env = TestEnv::new();
    assert_eq!(env.ingest_status().last_ingest_at, None);

    env.at(t0()).ingest();
    env.advance(Duration::minutes(7)).ingest();

    assert_eq!(
        env.ingest_status().last_ingest_at,
        Some(t0() + Duration::minutes(7))
    );
}

#[test]
fn the_ingest_status_tells_whether_anything_was_recorded() {
    let env = TestEnv::new();
    env.ingest();
    assert!(!env.ingest_status().has_data, "nothing recorded yet");

    env.hook_fixture("post_tool_use_bash.json");
    env.ingest();

    assert!(env.ingest_status().has_data);
}

#[test]
fn the_session_list_carries_each_sessions_tool_calls_and_time_split() {
    let env = TestEnv::new();
    env.drop_projects_fixture();
    env.replay_session_a_hooks();
    env.ingest();

    let sessions = env.sessions(&claudit::stats::Filter::default());
    let a = sessions
        .iter()
        .find(|s| s.session_id == "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f")
        .unwrap();

    // Bash, Agent, and the subagent's Read.
    assert_eq!(a.tool_calls, 3);
    // Turn 1: model 14.5 s, tools 4 s, waiting 11.8 s; turn 2: model
    // 16.8 s, waiting 0.1 s, subagent 63.4 s.
    assert_eq!(a.time.model, Duration::milliseconds(31_300));
    assert_eq!(a.time.tool, Duration::milliseconds(4_000));
    assert_eq!(a.time.waiting, Duration::milliseconds(11_900));
    assert_eq!(a.time.subagent, Duration::milliseconds(63_400));
}
