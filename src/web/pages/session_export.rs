//! A session's Markdown export (`/sessions/{id}/export?level=full`), as a
//! file download: the summary by default, the full export with
//! `level=full` (see [`crate::export`]).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::activities::ActivityRules;
use crate::db;
use crate::export::{self, ExportLevel};
use crate::pricing::PriceTable;
use crate::web::AppState;
use crate::web::error::WebError;

#[derive(Deserialize)]
pub(in crate::web) struct LevelParam {
    level: Option<String>,
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(param): Query<LevelParam>,
) -> Result<Response, WebError> {
    let level = match param.level.as_deref() {
        Some("full") => ExportLevel::Full,
        _ => ExportLevel::Summary,
    };
    let id = session_id.clone();
    let markdown = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let conn = db::open(&state.paths)?;
        let rules = ActivityRules::load(&state.paths).rules;
        export::session_markdown(&conn, &id, level, &rules, PriceTable::builtin())
    })
    .await??;
    let Some(markdown) = markdown else {
        return Ok((StatusCode::NOT_FOUND, "Unknown session").into_response());
    };
    // Session ids are UUIDs; keep the file name to safe characters anyway.
    let short: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(8)
        .collect();
    let disposition = format!(
        "attachment; filename=\"claudit-session-{short}-{}.md\"",
        level.name()
    );
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "text/markdown; charset=utf-8".to_owned(),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        markdown,
    )
        .into_response())
}
