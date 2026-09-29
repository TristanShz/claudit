//! The overview (home) page.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;
use serde::Serialize;

use crate::db;
use crate::stats::tools::{self, ToolStat};
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
}

impl From<ToolStat> for ToolRow {
    fn from(stat: ToolStat) -> Self {
        Self {
            total: format::duration_ms(stat.total_duration_ms),
            name: stat.tool_name,
            calls: stat.calls,
            total_ms: stat.total_duration_ms,
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
        tools::top_tools(&conn, &filter)
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
