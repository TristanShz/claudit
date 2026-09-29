//! Tracer bullet (#2): a tool call travels from the hook to the stats API.

mod common;

use chrono::Duration;
use claudit::stats::Filter;
use common::TestEnv;
use serde_json::json;

/// `tool_ranking` reduced to (tool, calls, total duration in ms).
fn tools(env: &TestEnv, filter: &Filter) -> Vec<(String, u64, u64)> {
    env.tool_ranking(filter)
        .into_iter()
        .map(|t| (t.name, t.stats.calls, t.stats.total_duration_ms))
        .collect()
}

#[test]
fn replayed_post_tool_use_appears_in_the_tool_ranking_with_its_duration() {
    let env = TestEnv::new();

    env.hook_fixture("post_tool_use_bash.json");
    env.ingest();

    assert_eq!(
        tools(&env, &Filter::default()),
        vec![("Bash".into(), 1, 4187)]
    );
}

#[test]
fn ingesting_twice_does_not_double_count() {
    let env = TestEnv::new();
    env.hook_fixture("post_tool_use_bash.json");

    env.ingest();
    let first = env.tool_ranking(&Filter::default());
    env.ingest();

    assert_eq!(env.tool_ranking(&Filter::default()), first);
    assert_eq!(first[0].stats.calls, 1);
}

#[test]
fn a_tool_call_delivered_twice_is_counted_once() {
    let env = TestEnv::new();

    env.hook_fixture("post_tool_use_bash.json");
    env.ingest();
    env.advance(chrono::Duration::milliseconds(3));
    env.hook_fixture("post_tool_use_bash.json");
    env.ingest();

    assert_eq!(
        tools(&env, &Filter::default()),
        vec![("Bash".into(), 1, 4187)]
    );
}

fn read_call(env: &TestEnv, id: &str, duration_ms: i64) {
    env.hook_fixture_with("post_tool_use_read.json", |p| {
        p["tool_use_id"] = json!(id);
        p["duration_ms"] = json!(duration_ms);
    });
}

#[test]
fn tools_are_ranked_by_call_count_with_summed_durations() {
    let env = TestEnv::new();
    env.hook_fixture("post_tool_use_bash.json");
    read_call(&env, "toolu_read_1", 10);
    read_call(&env, "toolu_read_2", 30);
    env.ingest();

    assert_eq!(
        tools(&env, &Filter::default()),
        vec![("Read".into(), 2, 40), ("Bash".into(), 1, 4187)]
    );
}

#[test]
fn the_tool_ranking_honours_the_date_range_and_project_filters() {
    let env = TestEnv::new();
    env.at(common::t0());
    read_call(&env, "toolu_monday", 10);
    env.advance(Duration::days(1));
    read_call(&env, "toolu_tuesday", 20);
    env.hook_fixture_with("post_tool_use_bash.json", |p| {
        p["cwd"] = json!("/Users/alice/code/other-project");
    });
    env.ingest();

    let tuesday = Filter {
        from: Some(common::t0() + Duration::hours(12)),
        to: Some(common::t0() + Duration::days(2)),
        ..Filter::default()
    };
    let names_and_totals = |filter: &Filter| tools(&env, filter);
    assert_eq!(
        names_and_totals(&tuesday),
        vec![("Bash".into(), 1, 4187), ("Read".into(), 1, 20)]
    );

    let acme = Filter {
        project: Some("/Users/alice/code/acme-api".into()),
        ..tuesday
    };
    assert_eq!(names_and_totals(&acme), vec![("Read".into(), 1, 20)]);

    let prefix_only = Filter {
        project: Some("/Users/alice/code/acme".into()),
        ..Filter::default()
    };
    assert_eq!(names_and_totals(&prefix_only), vec![]);
}
