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
use common::TestEnv;
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

#[tokio::test]
async fn every_page_renders_its_sections_on_the_fixture_archive() {
    let env = TestEnv::new();
    env.drop_projects_fixture();
    env.replay_session_a_hooks();
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
    ] {
        let (status, body) = get(&env, uri).await;
        assert_sections(uri, status, &body, ids);
    }
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
    env.drop_projects_fixture();
    env.replay_session_a_hooks();
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
