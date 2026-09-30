//! `claudit reingest` (#7): rebuilding every derived table from
//! `raw_events` plus the transcripts still on disk.
//!
//! Seams under test: seam 1 (hook payloads and transcripts in,
//! `claudit::ingest::run` / `claudit::ingest::reingest`, typed stats reports
//! out) and the `claudit reingest` binary. Equivalence is asserted on every
//! stats report at once through [`Reports`].

mod common;

use chrono::Duration;
use claudit::pricing::PriceTable;
use claudit::stats::{self, Filter};
use common::TestEnv;
use serde_json::json;

/// Every stats report, over the whole archive, filtered to one project and
/// for one session. A ticket adding a report adds it here, so reingest is
/// checked against it.
#[derive(Debug, PartialEq)]
struct Reports {
    tool_ranking: Vec<stats::tools::RankedCalls>,
    bash_command_ranking: Vec<stats::tools::RankedCalls>,
    mcp_server_ranking: Vec<stats::tools::RankedCalls>,
    consumption: stats::consumption::Consumption,
    sessions: Vec<stats::sessions::SessionSummary>,
    ingest_status: stats::ingest_status::IngestStatus,
    filter_options: stats::filter_options::FilterOptions,
    time_breakdown: stats::time::TimeBreakdown,
    turn_times: Vec<stats::time::TurnTime>,
    waiting_by_tool: Vec<stats::time::ToolWaiting>,
    skills: Vec<stats::skills::SkillStat>,
    subagents: Vec<stats::subagents::SubagentTypeStat>,
    subagent_runs: Vec<stats::subagents::SubagentRun>,
    total_cost: stats::cost::CostLine,
    cost_by_session: Vec<stats::cost::CostLine>,
    cost_by_model: Vec<stats::cost::CostLine>,
    cost_by_skill: Vec<stats::cost::CostLine>,
    cost_by_agent_type: Vec<stats::cost::CostLine>,
    daily_series: Vec<stats::cost::DailyUsage>,
    acme_tool_ranking: Vec<stats::tools::RankedCalls>,
    acme_consumption: stats::consumption::Consumption,
    acme_sessions: Vec<stats::sessions::SessionSummary>,
    acme_time_breakdown: stats::time::TimeBreakdown,
    acme_skills: Vec<stats::skills::SkillStat>,
    acme_subagents: Vec<stats::subagents::SubagentTypeStat>,
    acme_total_cost: stats::cost::CostLine,
    session_a_turn_times: Vec<stats::time::TurnTime>,
    session_a_skill_invocations: Vec<stats::skills::SkillInvocation>,
    session_a_subagent_runs: Vec<stats::subagents::SubagentRun>,
    session_a_detail: Option<stats::session_detail::SessionDetail>,
}

/// Fixture session `8d0c5a3e-…`, whose hooks [`populate`] replays.
const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";

fn reports(env: &TestEnv) -> Reports {
    let all = Filter::default();
    let acme = Filter {
        project: Some("/Users/alice/code/acme-api".to_owned()),
        ..Filter::default()
    };
    let conn = env.db();
    let prices = PriceTable::builtin();
    let cost = |f: fn(
        &rusqlite::Connection,
        &Filter,
        &PriceTable,
    ) -> anyhow::Result<Vec<stats::cost::CostLine>>| {
        f(&conn, &all, prices).expect("cost report")
    };
    Reports {
        tool_ranking: env.tool_ranking(&all),
        bash_command_ranking: env.bash_command_ranking(&all),
        mcp_server_ranking: env.mcp_server_ranking(&all),
        consumption: env.consumption(&all),
        sessions: env.sessions(&all),
        ingest_status: env.ingest_status(),
        filter_options: stats::filter_options::filter_options(&conn).expect("filter_options"),
        time_breakdown: env.time_breakdown(&all),
        turn_times: env.turn_times(&all),
        waiting_by_tool: env.waiting_by_tool(&all),
        skills: env.skills(&all),
        subagents: env.subagents(&all),
        subagent_runs: env.subagent_runs(&all),
        total_cost: stats::cost::total_cost(&conn, &all, prices).expect("total_cost"),
        cost_by_session: cost(stats::cost::cost_by_session),
        cost_by_model: cost(stats::cost::cost_by_model),
        cost_by_skill: cost(stats::cost::cost_by_skill),
        cost_by_agent_type: cost(stats::cost::cost_by_agent_type),
        daily_series: stats::cost::daily_series(&conn, &all, prices).expect("daily_series"),
        acme_tool_ranking: env.tool_ranking(&acme),
        acme_consumption: env.consumption(&acme),
        acme_sessions: env.sessions(&acme),
        acme_time_breakdown: env.time_breakdown(&acme),
        acme_skills: env.skills(&acme),
        acme_subagents: env.subagents(&acme),
        acme_total_cost: stats::cost::total_cost(&conn, &acme, prices).expect("total_cost"),
        session_a_turn_times: stats::time::session_turn_times(&conn, SESSION_A)
            .expect("session_turn_times"),
        session_a_skill_invocations: stats::skills::session_skill_invocations(&conn, SESSION_A)
            .expect("session_skill_invocations"),
        session_a_subagent_runs: stats::subagents::session_subagent_runs(&conn, SESSION_A)
            .expect("session_subagent_runs"),
        session_a_detail: stats::session_detail::session_detail(&conn, SESSION_A, prices)
            .expect("session_detail"),
    }
}

/// Hook events and transcripts for several sessions, including a duplicate
/// delivery, ingested over two runs.
fn populate(env: &TestEnv) {
    env.drop_projects_fixture();
    // Session A's hooks: turn times, waits, permission prompts, a skill and
    // a subagent run, matching its transcripts.
    env.replay_session_a_hooks();
    env.hook_fixture("post_tool_use_bash.json");
    env.advance(Duration::seconds(5));
    env.hook_fixture("post_tool_use_read.json");
    env.hook_fixture("post_tool_use_agent.json");
    env.ingest();
    env.advance(Duration::minutes(3));
    env.hook_fixture_with("post_tool_use_bash.json", |p| {
        p["tool_use_id"] = json!("toolu_01SecondBashCall000");
        p["duration_ms"] = json!(950);
    });
    // Duplicate delivery of an already ingested call.
    env.hook_fixture("post_tool_use_read.json");
    env.hook(&json!({
        "session_id": "3f2b8c1e-7d4a-4e5b-9c6f-1a2b3c4d5e6f",
        "cwd": "/Users/alice/code/acme-api",
        "hook_event_name": "Stop",
        "last_assistant_message": "All tests pass.",
    }));
    env.ingest();
}

#[test]
fn reingest_on_a_populated_archive_yields_identical_reports() {
    let env = TestEnv::new();
    populate(&env);
    let before = reports(&env);
    // Bash, Agent and Read from the hooks; Edit only from session
    // `2b7e4f10`'s transcript.
    assert_eq!(before.tool_ranking.len(), 4, "the archive is populated");
    assert!(before.consumption.sessions > 0, "transcripts were ingested");
    assert!(!before.turn_times.is_empty(), "turns were timed");
    assert!(!before.waiting_by_tool.is_empty(), "waits were measured");
    assert!(!before.skills.is_empty(), "skills were invoked");
    assert!(!before.subagent_runs.is_empty(), "subagents ran");
    assert!(
        !before.cost_by_agent_type.is_empty(),
        "subagents were priced"
    );
    assert!(!before.session_a_skill_invocations.is_empty());
    assert!(before.session_a_detail.is_some());

    claudit::ingest::reingest(&env.paths).expect("reingest succeeds");

    assert_eq!(reports(&env), before);
}

#[test]
fn reingesting_twice_never_double_counts() {
    let env = TestEnv::new();
    populate(&env);
    let before = reports(&env);

    claudit::ingest::reingest(&env.paths).expect("first reingest");
    claudit::ingest::reingest(&env.paths).expect("second reingest");
    env.ingest();

    assert_eq!(reports(&env), before);
}

#[test]
fn reingest_also_ingests_what_is_still_pending() {
    let plain = TestEnv::new();
    populate(&plain);
    plain.hook_fixture_with("post_tool_use_bash.json", |p| {
        p["tool_use_id"] = json!("toolu_01PendingBashCall00");
    });
    plain.ingest();
    let mut after_plain_ingest = reports(&plain);

    let pending = TestEnv::new();
    populate(&pending);
    pending.hook_fixture_with("post_tool_use_bash.json", |p| {
        p["tool_use_id"] = json!("toolu_01PendingBashCall00");
    });
    let report = claudit::ingest::reingest(&pending.paths).expect("reingest succeeds");

    assert_eq!(report.ingest.events, 1);
    let mut after_reingest = reports(&pending);
    // The backfill time is wall-clock and differs between the two archives.
    after_plain_ingest.ingest_status.transcripts_backfilled_at = None;
    after_reingest.ingest_status.transcripts_backfilled_at = None;
    assert_eq!(after_reingest, after_plain_ingest);
}

#[test]
fn hook_data_survives_a_reingest_after_its_transcripts_are_gone() {
    let env = TestEnv::new();
    populate(&env);
    // A call known only from a transcript goes with it, as its tokens do.
    let tools: Vec<_> = env
        .tool_ranking(&Filter::default())
        .into_iter()
        .filter(|tool| tool.stats.estimated_duration_calls == 0)
        .collect();
    assert_eq!(tools.len(), 3, "only Edit was transcript-only");
    std::fs::remove_dir_all(env.paths.claude_projects_dir()).unwrap();

    claudit::ingest::reingest(&env.paths).expect("reingest succeeds");

    assert_eq!(env.tool_ranking(&Filter::default()), tools);
    assert_eq!(env.consumption(&Filter::default()).tokens.total(), 0);
}

#[test]
fn the_reingest_command_rebuilds_the_archive() {
    let env = TestEnv::new();
    populate(&env);
    let before = reports(&env);

    let out = env.run_bin(&["reingest"], b"");

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("replayed 23 archived events"), "{stdout}");
    assert_eq!(reports(&env), before);
}
