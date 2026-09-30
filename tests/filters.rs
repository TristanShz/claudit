//! The global filter (#11): every stats report honours every filter
//! dimension — date range, project, branch and model.
//!
//! Seam under test: seam 1 (hook payloads and transcripts in, typed
//! `claudit::stats` reports out). Each test runs **every** filtered report
//! under one filter and compares a digest of all of them ([`Digest`]) with
//! an expectation worked out by hand from the fixtures.
//!
//! Per-report semantics (also documented on each report):
//! - project and branch match the session's working directory and git
//!   branch (a tool call's own `cwd` wins over its session's);
//! - model matches API messages of that model, and everything happening in
//!   a turn (or subagent thread) that used it: its tool calls, waits, skill
//!   invocations, subagent runs; sessions and turns that used it;
//! - the date range applies to each report's own event time (session start,
//!   turn start, tool completion, API response, run start).

mod common;

use chrono::{Duration, TimeZone, Utc};
use claudit::pricing::PriceTable;
use claudit::stats::{self, Filter};
use common::{TestEnv, t0};
use serde_json::json;

const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";
const LOGIN: &str = "2b7e4f10-3c5d-4e6f-8a9b-1c2d3e4f5a6b";
const WEB_APP: &str = "5c9d1e22-7a8b-4c9d-8e0f-2a3b4c5d6e7f";
const HAIKU: &str = "claude-haiku-4-5-20251001";

/// A comparable digest of every filtered report.
#[derive(Debug, Default, PartialEq)]
struct Digest {
    /// `tool_ranking`: (tool, calls).
    tools: Vec<(String, u64)>,
    bash_commands: Vec<String>,
    mcp_servers: Vec<String>,
    /// `consumption`: sessions, turns, total tokens.
    consumption: (u64, u64, u64),
    session_list: Vec<String>,
    /// `turn_times`: session of each turn.
    turn_times: Vec<String>,
    time_breakdown_turns: u64,
    waiting_by_tool: Vec<String>,
    skills: Vec<String>,
    subagents: Vec<String>,
    subagent_runs: Vec<String>,
    total_cost_tokens: u64,
    cost_by_session: Vec<String>,
    cost_by_model: Vec<String>,
    cost_by_skill: Vec<String>,
    cost_by_agent_type: Vec<String>,
    daily_series_days: Vec<String>,
    /// `model_usage`: models, as ranked.
    models: Vec<String>,
}

fn digest(env: &TestEnv, filter: &Filter) -> Digest {
    let conn = env.db();
    let prices = PriceTable::builtin();
    let keys = |lines: Vec<stats::cost::CostLine>| {
        let mut keys: Vec<String> = lines.into_iter().map(|l| l.key).collect();
        keys.sort();
        keys
    };
    let consumption = stats::consumption::consumption(&conn, filter).unwrap();
    let mut days: Vec<String> = stats::cost::daily_series(&conn, filter, prices)
        .unwrap()
        .into_iter()
        .map(|d| d.day.to_string())
        .collect();
    days.dedup();
    Digest {
        tools: stats::tools::tool_ranking(&conn, filter)
            .unwrap()
            .into_iter()
            .map(|r| (r.name, r.stats.calls))
            .collect(),
        bash_commands: names(stats::tools::bash_command_ranking(&conn, filter).unwrap()),
        mcp_servers: names(stats::tools::mcp_server_ranking(&conn, filter).unwrap()),
        consumption: (
            consumption.sessions,
            consumption.turns,
            consumption.tokens.total(),
        ),
        session_list: stats::sessions::session_list(&conn, filter)
            .unwrap()
            .into_iter()
            .map(|s| s.session_id)
            .collect(),
        turn_times: stats::time::turn_times(&conn, filter)
            .unwrap()
            .into_iter()
            .map(|t| t.session_id)
            .collect(),
        time_breakdown_turns: stats::time::time_breakdown(&conn, filter).unwrap().turns,
        waiting_by_tool: stats::time::waiting_by_tool(&conn, filter)
            .unwrap()
            .into_iter()
            .map(|w| w.tool_name)
            .collect(),
        skills: stats::skills::skill_ranking(&conn, filter)
            .unwrap()
            .into_iter()
            .map(|s| s.skill)
            .collect(),
        subagents: stats::subagents::subagent_ranking(&conn, filter)
            .unwrap()
            .into_iter()
            .map(|s| s.agent_type)
            .collect(),
        subagent_runs: stats::subagents::subagent_runs(&conn, filter)
            .unwrap()
            .into_iter()
            .map(|r| r.agent_id)
            .collect(),
        total_cost_tokens: stats::cost::total_cost(&conn, filter, prices)
            .unwrap()
            .tokens
            .total(),
        cost_by_session: keys(stats::cost::cost_by_session(&conn, filter, prices).unwrap()),
        cost_by_model: keys(stats::cost::cost_by_model(&conn, filter, prices).unwrap()),
        cost_by_skill: keys(stats::cost::cost_by_skill(&conn, filter, prices).unwrap()),
        cost_by_agent_type: keys(stats::cost::cost_by_agent_type(&conn, filter, prices).unwrap()),
        daily_series_days: days,
        models: stats::models::model_usage(&conn, filter, prices)
            .unwrap()
            .models
            .into_iter()
            .map(|m| m.model)
            .collect(),
    }
}

fn names(rows: Vec<stats::tools::RankedCalls>) -> Vec<String> {
    rows.into_iter().map(|r| r.name).collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// The fixture archive:
/// - session A (`main`, Opus, acme-api, 2026-03-02) with its hooks: a Bash
///   call behind a permission prompt, a typed skill, a Haiku subagent
///   making one Read call;
/// - session `2b7e4f10` (`feat/login`, Sonnet, acme-api, 2026-03-03) plus one
///   Bash call (`git status`);
/// - session `5c9d1e22` (`main`, Opus, web-app, 2026-03-04) plus one Read;
/// - session `3f2b8c1e`, known only from hooks (no transcript, hence no
///   branch nor model): Bash, Read and an MCP call in acme-api.
fn populate(env: &TestEnv) {
    env.drop_projects_fixture();
    env.replay_session_a_hooks();
    env.at(t0() + Duration::hours(1));
    for name in [
        "post_tool_use_bash.json",
        "post_tool_use_read.json",
        "post_tool_use_mcp.json",
    ] {
        env.hook_fixture(name);
    }
    env.at(Utc.with_ymd_and_hms(2026, 3, 3, 14, 0, 3).unwrap())
        .hook_fixture_with("post_tool_use_bash.json", |p| {
            p["session_id"] = json!(LOGIN);
            p["prompt_id"] = json!("a1b2c3d4-0003-4000-8000-000000000003");
            p["tool_use_id"] = json!("toolu_01LoginBash000001");
            p["tool_input"]["command"] = json!("git status");
        });
    env.at(Utc.with_ymd_and_hms(2026, 3, 4, 10, 0, 2).unwrap())
        .hook_fixture_with("post_tool_use_read.json", |p| {
            p["session_id"] = json!(WEB_APP);
            p["prompt_id"] = json!("a1b2c3d4-0004-4000-8000-000000000004");
            p["tool_use_id"] = json!("toolu_01WebAppRead00001");
            p["cwd"] = json!("/Users/alice/code/web-app");
            p["tool_input"]["file_path"] = json!("/Users/alice/code/web-app/index.ts");
        });
    env.ingest();
}

/// Everything session `2b7e4f10` (`feat/login`, 2026-03-03) contributes.
fn login_session_only() -> Digest {
    Digest {
        tools: vec![("Bash".to_owned(), 1)],
        bash_commands: strings(&["git"]),
        // Tokens: 5 + 150 + 3200 + 3000.
        consumption: (1, 1, 6355),
        session_list: strings(&[LOGIN]),
        turn_times: strings(&[LOGIN]),
        time_breakdown_turns: 1,
        total_cost_tokens: 6355,
        cost_by_session: strings(&[LOGIN]),
        cost_by_model: strings(&["claude-sonnet-4-6"]),
        daily_series_days: strings(&["2026-03-03"]),
        models: strings(&["claude-sonnet-4-6"]),
        ..Digest::default()
    }
}

#[test]
fn a_branch_filter_keeps_only_that_branchs_sessions_in_every_report() {
    let env = TestEnv::new();
    populate(&env);

    let filter = Filter {
        branch: Some("feat/login".to_owned()),
        ..Filter::default()
    };

    assert_eq!(digest(&env, &filter), login_session_only());
}

#[test]
fn a_date_range_keeps_only_what_happened_in_it_in_every_report() {
    let env = TestEnv::new();
    populate(&env);

    let filter = Filter {
        from: Some(Utc.with_ymd_and_hms(2026, 3, 3, 0, 0, 0).unwrap()),
        to: Some(Utc.with_ymd_and_hms(2026, 3, 4, 0, 0, 0).unwrap()),
        ..Filter::default()
    };

    assert_eq!(digest(&env, &filter), login_session_only());
}

#[test]
fn a_project_filter_keeps_only_that_directory_in_every_report() {
    let env = TestEnv::new();
    populate(&env);

    let filter = Filter {
        project: Some("/Users/alice/code/web-app".to_owned()),
        ..Filter::default()
    };

    assert_eq!(
        digest(&env, &filter),
        Digest {
            tools: vec![("Read".to_owned(), 1)],
            // Tokens: 2 + 500 + 5000 + 1000.
            consumption: (1, 1, 6502),
            session_list: strings(&[WEB_APP]),
            turn_times: strings(&[WEB_APP]),
            time_breakdown_turns: 1,
            total_cost_tokens: 6502,
            cost_by_session: strings(&[WEB_APP]),
            cost_by_model: strings(&["claude-opus-5-5"]),
            daily_series_days: strings(&["2026-03-04"]),
            models: strings(&["claude-opus-5-5"]),
            ..Digest::default()
        }
    );
}

#[test]
fn a_model_filter_keeps_what_that_model_did_in_every_report() {
    let env = TestEnv::new();
    populate(&env);

    let filter = Filter {
        model: Some(HAIKU.to_owned()),
        ..Filter::default()
    };

    // Only the subagent of session A's second turn ran on Haiku: its Read
    // call (which waited 0.1 s to start), its run, its tokens; the session
    // and the turn it ran in used Haiku.
    assert_eq!(
        digest(&env, &filter),
        Digest {
            tools: vec![("Read".to_owned(), 1)],
            // Tokens: 6 + 300 + 4300 + 4000.
            consumption: (1, 1, 8606),
            session_list: strings(&[SESSION_A]),
            turn_times: strings(&[SESSION_A]),
            time_breakdown_turns: 1,
            waiting_by_tool: strings(&["Read"]),
            subagents: strings(&["general-purpose"]),
            subagent_runs: strings(&["a1f3c5e7b9d2c4e6f"]),
            total_cost_tokens: 8606,
            cost_by_session: strings(&[SESSION_A]),
            cost_by_model: strings(&[HAIKU]),
            cost_by_agent_type: strings(&["general-purpose"]),
            daily_series_days: strings(&["2026-03-02"]),
            models: strings(&[HAIKU]),
            ..Digest::default()
        }
    );
}

#[test]
fn a_branch_filter_leaves_out_tool_calls_of_sessions_without_a_known_branch() {
    let env = TestEnv::new();
    populate(&env);

    let filter = Filter {
        branch: Some("main".to_owned()),
        ..Filter::default()
    };

    // Session A's calls and web-app's Read; none of session 3f2b8c1e's.
    assert_eq!(
        digest(&env, &filter).tools,
        vec![
            ("Read".to_owned(), 2),
            ("Agent".to_owned(), 1),
            ("Bash".to_owned(), 1),
        ]
    );
}

#[test]
fn a_filter_matching_nothing_empties_every_report() {
    let env = TestEnv::new();
    populate(&env);

    for filter in [
        Filter {
            from: Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()),
            ..Filter::default()
        },
        Filter {
            to: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            ..Filter::default()
        },
        Filter {
            project: Some("/Users/alice/code/nowhere".to_owned()),
            ..Filter::default()
        },
        Filter {
            branch: Some("no-such-branch".to_owned()),
            ..Filter::default()
        },
        Filter {
            model: Some("no-such-model".to_owned()),
            ..Filter::default()
        },
    ] {
        assert_eq!(digest(&env, &filter), Digest::default(), "{filter:?}");
    }
}
