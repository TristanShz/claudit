//! Smoke check of the dashboard pages (#11, #12): each page renders (HTTP
//! 200) with its sections, on an empty archive and on the fixture archive.
//!
//! The spec keeps tests off the HTTP layer and templates (they are thin
//! adapters over the stats API, tested elsewhere); this file only guards
//! against a page failing to render, and asserts nothing but the status and
//! the presence of section ids.
//!
//! Seam under test: `claudit::web::router`, called in process.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Duration;
use common::{TestEnv, t0};
use serde_json::json;
use tower::ServiceExt;

/// GETs `uri` and returns the status and body.
async fn get(env: &TestEnv, uri: &str) -> (StatusCode, String) {
    let response = claudit::web::router(env.paths.clone())
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

fn assert_sections(uri: &str, status: StatusCode, body: &str, ids: &[&str]) {
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    for id in ids {
        assert!(body.contains(&format!("id=\"{id}\"")), "{uri} lacks #{id}");
    }
}

#[tokio::test]
async fn the_overview_shows_the_empty_state_on_an_empty_archive() {
    let env = TestEnv::new();
    env.ingest();

    let (status, body) = get(&env, "/").await;

    assert_sections(
        "/",
        status,
        &body,
        &["filter-bar", "last-ingest", "empty-state"],
    );
    assert!(body.contains("claudit install"));
    assert!(!body.contains("id=\"kpis-section\""));
}

const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";

/// The fixture sessions, session A's hooks, and a Read call running in
/// parallel with session A's first Bash call.
fn populate(env: &TestEnv) {
    env.drop_projects_fixture();
    env.replay_session_a_hooks();
    let parallel = |p: &mut serde_json::Value| {
        p["session_id"] = json!(SESSION_A);
        p["prompt_id"] = json!("a1b2c3d4-0001-4000-8000-000000000001");
        p["tool_name"] = json!("Read");
        p["tool_use_id"] = json!("toolu_01AcmeParallelRead");
        p["tool_input"] = json!({ "file_path": "/Users/alice/code/acme-api/Cargo.toml" });
    };
    env.at(t0() + Duration::milliseconds(18_000))
        .hook_fixture_with("pre_tool_use_bash.json", parallel);
    env.at(t0() + Duration::milliseconds(22_000))
        .hook_fixture_with("post_tool_use_read.json", |p| {
            parallel(p);
            p["duration_ms"] = json!(4000);
        });
}

#[tokio::test]
async fn every_page_renders_its_sections_on_the_fixture_archive() {
    let env = TestEnv::new();
    populate(&env);
    env.ingest();

    for (uri, ids) in [
        (
            "/",
            &[
                "filter-bar",
                "kpis-section",
                "time-section",
                "waiting-section",
                "tools-section",
                "skills-section",
                "subagents-section",
                "sessions-section",
                "ingest-warning",
            ][..],
        ),
        (
            "/?branch=main&model=claude-opus-5-5&project=%2FUsers%2Falice%2Fcode%2Facme-api&from=2026-03-01&to=2026-03-31",
            &["kpis-section", "sessions-section"][..],
        ),
        (
            "/tools",
            &["tools-section", "bash-section", "mcp-section"][..],
        ),
        ("/skills", &["skills-section"][..]),
        ("/subagents", &["subagents-section", "runs-section"][..]),
        ("/sessions", &["sessions-section"][..]),
        (
            "/sessions/8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f?branch=main",
            &[
                "session-header",
                "session-kpis",
                "timeline-section",
                "timeline-data",
                "session-tools-section",
                "session-skills-subagents-section",
            ][..],
        ),
    ] {
        let (status, body) = get(&env, uri).await;
        assert_sections(uri, status, &body, ids);
    }
    let (status, _) = get(&env, "/sessions/00000000-0000-0000-0000-000000000000").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Not a test: builds a demo archive from the fixtures (for screenshots and
/// manual browser checks) and copies it to `$CLAUDIT_DEMO_HOME`:
/// `CLAUDIT_DEMO_HOME=/tmp/demo cargo test --test web_pages -- --ignored`,
/// then `CLAUDIT_HOME=/tmp/demo claudit serve`.
#[test]
#[ignore]
fn demo_archive() {
    let Ok(target) = std::env::var("CLAUDIT_DEMO_HOME") else {
        return;
    };
    let env = TestEnv::new();
    populate(&env);
    for name in [
        "post_tool_use_bash.json",
        "post_tool_use_read.json",
        "post_tool_use_mcp.json",
        "post_tool_use_failure_bash.json",
        "post_tool_use_skill.json",
    ] {
        env.hook_fixture(name);
    }
    env.ingest();
    let target = std::path::Path::new(&target);
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(env.paths.home()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}
