//! `claudit reingest` (#7): rebuilding every derived table from
//! `raw_events` plus the transcripts still on disk.
//!
//! Seams under test: seam 1 (hook payloads and transcripts in,
//! `claudit::ingest::run` / `claudit::ingest::reingest`, typed stats reports
//! out) and the `claudit reingest` binary. Equivalence is asserted on every
//! stats report at once through [`Reports`].

mod common;

use chrono::Duration;
use claudit::stats::{self, Filter};
use common::TestEnv;
use serde_json::json;

/// Every stats report, over the whole archive and filtered to one project.
/// A ticket adding a report adds it here, so reingest is checked against it.
#[derive(Debug, PartialEq)]
struct Reports {
    top_tools: Vec<stats::tools::ToolStat>,
    consumption: stats::consumption::Consumption,
    sessions: Vec<stats::sessions::SessionSummary>,
    ingest_status: stats::ingest_status::IngestStatus,
    acme_top_tools: Vec<stats::tools::ToolStat>,
    acme_consumption: stats::consumption::Consumption,
    acme_sessions: Vec<stats::sessions::SessionSummary>,
}

fn reports(env: &TestEnv) -> Reports {
    let all = Filter::default();
    let acme = Filter {
        project: Some("/Users/alice/code/acme-api".to_owned()),
        ..Filter::default()
    };
    Reports {
        top_tools: env.top_tools(&all),
        consumption: env.consumption(&all),
        sessions: env.sessions(&all),
        ingest_status: env.ingest_status(),
        acme_top_tools: env.top_tools(&acme),
        acme_consumption: env.consumption(&acme),
        acme_sessions: env.sessions(&acme),
    }
}

/// Hook events and transcripts for several sessions, including a duplicate
/// delivery, ingested over two runs.
fn populate(env: &TestEnv) {
    env.drop_projects_fixture();
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
    assert_eq!(before.top_tools.len(), 3, "the archive is populated");
    assert!(before.consumption.sessions > 0, "transcripts were ingested");

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
    let tools = env.top_tools(&Filter::default());
    std::fs::remove_dir_all(env.paths.claude_projects_dir()).unwrap();

    claudit::ingest::reingest(&env.paths).expect("reingest succeeds");

    assert_eq!(env.top_tools(&Filter::default()), tools);
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
    assert!(stdout.contains("replayed 6 archived events"), "{stdout}");
    assert_eq!(reports(&env), before);
}
