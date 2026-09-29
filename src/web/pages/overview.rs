//! The overview (home) page.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;
use serde::Serialize;

use crate::db;
use crate::stats::tools::{self, RankedCalls};
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;

#[derive(Template)]
#[template(path = "pages/overview.html")]
struct OverviewPage {
    filters: FilterParams,
    tools: Vec<ToolRow>,
    tools_chart_json: String,
}

/// A row of the tools section.
#[derive(Serialize)]
struct ToolRow {
    name: String,
    calls: u64,
    total_ms: u64,
    total: String,
    median: String,
    p95: String,
    failure_rate: String,
}

impl From<RankedCalls> for ToolRow {
    fn from(row: RankedCalls) -> Self {
        let stats = row.stats;
        let optional = |ms: Option<u64>| ms.map_or_else(|| "–".to_owned(), format::duration_ms);
        Self {
            total: format::duration_ms(stats.total_duration_ms),
            median: optional(stats.median_duration_ms),
            p95: optional(stats.p95_duration_ms),
            failure_rate: format::percent(stats.failure_rate()),
            name: row.name,
            calls: stats.calls,
            total_ms: stats.total_duration_ms,
        }
    }
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<FilterParams>,
) -> Result<Html<String>, WebError> {
    let filter = filters.to_filter();
    let tools: Vec<ToolRow> = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let conn = db::open(&state.paths)?;
        tools::tool_ranking(&conn, &filter)
    })
    .await??
    .into_iter()
    .map(ToolRow::from)
    .collect();

    let page = OverviewPage {
        tools_chart_json: format::script_json(&tools)?,
        filters,
        tools,
    };
    Ok(Html(page.render()?))
}
