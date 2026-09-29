//! Tool breakdowns (#6): failures, Bash leading commands, MCP servers and
//! duration percentiles.
//!
//! Seam under test: seam 1 only. Hook payloads (fixtures, varied through
//! `hook_fixture_with`) go in through the hook entry point, `TestEnv::ingest`
//! loads them, and assertions are made on the typed rankings of
//! `claudit::stats::tools`.

mod common;

use chrono::Duration;
use claudit::stats::Filter;
use claudit::stats::tools::{CallStats, RankedCalls};
use common::TestEnv;
use serde_json::json;

fn bash(env: &TestEnv, id: &str, command: &str, duration_ms: u64) {
    env.hook_fixture_with("post_tool_use_bash.json", |p| {
        p["tool_use_id"] = json!(id);
        p["tool_input"]["command"] = json!(command);
        p["duration_ms"] = json!(duration_ms);
    });
}

fn failed_bash(env: &TestEnv, id: &str, command: &str, duration_ms: u64) {
    env.hook_fixture_with("post_tool_use_failure_bash.json", |p| {
        p["tool_use_id"] = json!(id);
        p["tool_input"]["command"] = json!(command);
        p["duration_ms"] = json!(duration_ms);
    });
}

fn read(env: &TestEnv, id: &str, duration_ms: u64) {
    env.hook_fixture_with("post_tool_use_read.json", |p| {
        p["tool_use_id"] = json!(id);
        p["duration_ms"] = json!(duration_ms);
    });
}

#[test]
fn failed_calls_count_toward_the_tool_failure_rate() {
    let env = TestEnv::new();
    bash(&env, "toolu_ok_1", "cargo build", 100);
    bash(&env, "toolu_ok_2", "cargo build", 300);
    bash(&env, "toolu_ok_3", "cargo build", 200);
    failed_bash(&env, "toolu_ko_1", "pnpm test", 400);
    read(&env, "toolu_read", 12);
    env.ingest();

    let ranking = env.tool_ranking(&Filter::default());

    assert_eq!(
        ranking,
        vec![
            RankedCalls {
                name: "Bash".into(),
                stats: CallStats {
                    calls: 4,
                    failures: 1,
                    total_duration_ms: 1000,
                    median_duration_ms: Some(200),
                    p95_duration_ms: Some(400),
                },
            },
            RankedCalls {
                name: "Read".into(),
                stats: CallStats {
                    calls: 1,
                    failures: 0,
                    total_duration_ms: 12,
                    median_duration_ms: Some(12),
                    p95_duration_ms: Some(12),
                },
            },
        ]
    );
    assert_eq!(ranking[0].stats.failure_rate(), 0.25);
    assert_eq!(ranking[1].stats.failure_rate(), 0.0);
}

#[test]
fn median_and_p95_use_the_nearest_rank_method() {
    let env = TestEnv::new();
    // Twenty Read calls taking 10, 20, …, 200 ms, delivered out of order.
    for (i, tenth) in [
        7, 19, 2, 14, 20, 1, 11, 5, 16, 9, 3, 18, 12, 6, 15, 8, 13, 4, 17, 10,
    ]
    .into_iter()
    .enumerate()
    {
        read(&env, &format!("toolu_read_{i}"), tenth * 10);
    }
    // Seven Bash calls taking 1..=7 s, plus one that reports no duration:
    // it counts as a call but not in the percentiles.
    for seconds in [4, 7, 1, 6, 2, 5, 3] {
        bash(
            &env,
            &format!("toolu_bash_{seconds}"),
            "cargo test",
            seconds * 1000,
        );
    }
    env.hook_fixture_with("post_tool_use_bash.json", |p| {
        p["tool_use_id"] = json!("toolu_bash_untimed");
        p.as_object_mut().unwrap().remove("duration_ms");
    });
    env.ingest();

    let ranking = env.tool_ranking(&Filter::default());

    let read = ranking.iter().find(|row| row.name == "Read").unwrap();
    assert_eq!(read.stats.calls, 20);
    // n = 20: median is rank ceil(10) = 10th value, p95 rank ceil(19) = 19th.
    assert_eq!(read.stats.median_duration_ms, Some(100));
    assert_eq!(read.stats.p95_duration_ms, Some(190));

    let bash = ranking.iter().find(|row| row.name == "Bash").unwrap();
    assert_eq!(bash.stats.calls, 8);
    assert_eq!(bash.stats.total_duration_ms, 28_000);
    // n = 7: median is rank ceil(3.5) = 4th value, p95 rank ceil(6.65) = 7th.
    assert_eq!(bash.stats.median_duration_ms, Some(4000));
    assert_eq!(bash.stats.p95_duration_ms, Some(7000));
}

#[test]
fn bash_and_mcp_rankings_honour_the_date_range_and_project_filters() {
    let env = TestEnv::new();
    bash(&env, "toolu_monday_git", "git status", 10);
    mcp(&env, "toolu_monday_mcp", "mcp__github__get_issue", 10);
    env.advance(Duration::days(1));
    bash(&env, "toolu_tuesday_cargo", "cargo test", 10);
    mcp(&env, "toolu_tuesday_mcp", "mcp__linear__list_issues", 10);
    env.hook_fixture_with("post_tool_use_bash.json", |p| {
        p["tool_use_id"] = json!("toolu_other_project");
        p["cwd"] = json!("/Users/alice/code/other-project");
    });
    env.ingest();

    let tuesday_in_acme = Filter {
        from: Some(common::t0() + Duration::hours(12)),
        project: Some("/Users/alice/code/acme-api".into()),
        ..Filter::default()
    };
    let names = |rows: Vec<RankedCalls>| rows.into_iter().map(|r| r.name).collect::<Vec<_>>();

    assert_eq!(names(env.bash_command_ranking(&tuesday_in_acme)), ["cargo"]);
    assert_eq!(names(env.mcp_server_ranking(&tuesday_in_acme)), ["linear"]);
    assert_eq!(
        names(env.bash_command_ranking(&Filter::default())),
        ["cargo", "git"]
    );
}

fn mcp(env: &TestEnv, id: &str, tool_name: &str, duration_ms: u64) {
    env.hook_fixture_with("post_tool_use_mcp.json", |p| {
        p["tool_use_id"] = json!(id);
        p["tool_name"] = json!(tool_name);
        p["duration_ms"] = json!(duration_ms);
    });
}

#[test]
fn mcp_calls_are_grouped_by_server() {
    let env = TestEnv::new();
    mcp(&env, "toolu_1", "mcp__github__get_issue", 600);
    mcp(&env, "toolu_2", "mcp__github__get_issue", 700);
    env.hook_fixture_with("post_tool_use_mcp.json", |p| {
        p["hook_event_name"] = json!("PostToolUseFailure");
        p["tool_use_id"] = json!("toolu_3");
        p["tool_name"] = json!("mcp__github__create_pull_request");
        p["error"] = json!("MCP error -32603: Validation Failed");
        p["duration_ms"] = json!(900);
    });
    mcp(
        &env,
        "toolu_4",
        "mcp__plugin_context7_context7__query-docs",
        2000,
    );
    env.hook_fixture("post_tool_use_bash.json");
    env.ingest();

    let ranking = env.mcp_server_ranking(&Filter::default());

    assert_eq!(
        ranking,
        vec![
            RankedCalls {
                name: "github".into(),
                stats: CallStats {
                    calls: 3,
                    failures: 1,
                    total_duration_ms: 2200,
                    median_duration_ms: Some(700),
                    p95_duration_ms: Some(900),
                },
            },
            RankedCalls {
                name: "plugin_context7_context7".into(),
                stats: CallStats {
                    calls: 1,
                    failures: 0,
                    total_duration_ms: 2000,
                    median_duration_ms: Some(2000),
                    p95_duration_ms: Some(2000),
                },
            },
        ]
    );
}

#[test]
fn bash_calls_are_ranked_by_leading_command() {
    let env = TestEnv::new();
    let commands = [
        "git status",
        "/usr/bin/git log --oneline",
        "git add -A && git commit -m 'fix: a; b | c'",
        "FOO=1 BAR=\"a b\" cargo test",
        "env RUST_LOG=debug cargo run",
        "time cargo build --release",
        "cd /Users/alice/code/acme-api && cargo test",
        "sudo npm install",
        "nohup pnpm dev &",
        "  pnpm lint; pnpm test",
        "(cd web && pnpm build)",
        "cat Cargo.toml | grep version",
        "cd sub 2>&1",
        "",
    ];
    for (i, command) in commands.iter().enumerate() {
        bash(&env, &format!("toolu_{i}"), command, 10);
    }
    failed_bash(&env, "toolu_failed", "git push", 10);
    read(&env, "toolu_read", 12);
    env.ingest();

    let ranking = env.bash_command_ranking(&Filter::default());

    let summary: Vec<_> = ranking
        .iter()
        .map(|row| (row.name.as_str(), row.stats.calls, row.stats.failures))
        .collect();
    assert_eq!(
        summary,
        [
            ("cargo", 4, 0),
            ("git", 4, 1),
            ("pnpm", 3, 0),
            ("cat", 1, 0),
            ("cd", 1, 0),
            ("npm", 1, 0),
        ]
    );
}
