//! The `claudit hook` process contract: Claude Code must never see a
//! claudit failure, so the hook always exits 0, never prints, and reports
//! problems in the claudit log instead.

mod common;

use claudit::stats::Filter;
use common::{TestEnv, hook_fixture};

#[test]
fn hook_binary_spools_a_payload_silently() {
    let env = TestEnv::new();
    let payload = hook_fixture("post_tool_use_bash.json").to_string();

    let out = env.run_bin(&["hook"], payload.as_bytes());

    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    env.ingest();
    assert_eq!(env.tool_ranking(&Filter::default())[0].name, "Bash");
    assert_eq!(env.log(), "");
}

#[test]
fn hook_binary_exits_zero_and_prints_nothing_on_malformed_stdin() {
    let env = TestEnv::new();

    let out = env.run_bin(&["hook"], b"{ this is not json");

    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    assert!(out.stderr.is_empty(), "stderr: {:?}", out.stderr);
    assert!(
        env.log().contains("hook payload is not valid JSON"),
        "log: {}",
        env.log()
    );
    env.ingest();
    assert!(env.ingest_status().has_data, "the raw payload was archived");
}

#[test]
fn a_payload_without_session_id_is_logged_not_spooled() {
    let env = TestEnv::new();

    env.hook_fixture_with("post_tool_use_bash.json", |p| {
        p.as_object_mut().unwrap().remove("session_id");
    });
    env.ingest();

    assert!(env.log().contains("session_id"), "log: {}", env.log());
    assert_eq!(env.tool_ranking(&Filter::default()), vec![]);
}

#[test]
fn a_session_id_that_is_not_a_safe_file_name_is_rejected() {
    let env = TestEnv::new();

    env.hook_fixture_with("post_tool_use_bash.json", |p| {
        p["session_id"] = serde_json::json!("../../escape");
    });
    env.ingest();

    assert!(
        env.log().contains("not a valid identifier"),
        "log: {}",
        env.log()
    );
    assert_eq!(env.tool_ranking(&Filter::default()), vec![]);
}

#[test]
fn an_unparsable_payload_is_spooled_raw_and_archived_without_projection() {
    let env = TestEnv::new();

    env.hook_raw(b"{\"hook_event_name\": \"PostToolUse\", truncated");
    let report = env.ingest_report();

    assert!(
        env.log().contains("hook payload is not valid JSON"),
        "log: {}",
        env.log()
    );
    assert_eq!(report.events, 1, "archived: {report:?}");
    assert_eq!(report.unprojected_events, 1, "counted: {report:?}");
    assert_eq!(report.skipped_lines, 0, "{report:?}");
    assert_eq!(env.tool_ranking(&Filter::default()), vec![]);
    assert!(env.ingest_status().has_data);
}
