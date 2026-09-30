//! Smoke check of the dashboard pages (#11, #12): each page renders (HTTP
//! 200) with its sections, on an empty archive and on the fixture archive.
//!
//! The one sanctioned exception to "no tests on the HTTP layer or the
//! templates" (CONTRIBUTING.md): tickets #11 and #12 require each page to
//! render, so this file only guards against a page failing to render. It
//! asserts nothing but the status and the presence of section ids, never
//! page content.
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
    env.populate_fixture_archive();
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
                "models-section",
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
        ("/models", &["models-section", "models-threads-section"][..]),
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
