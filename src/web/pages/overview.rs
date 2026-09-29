//! The overview (home) page.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;
use serde::Serialize;

use crate::db;
use crate::stats::consumption::{self, Consumption};
use crate::stats::ingest_status::{self, IngestStatus};
use crate::stats::tools::{self, RankedCalls};
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;

#[derive(Template)]
#[template(path = "pages/overview.html")]
struct OverviewPage {
    filters: FilterParams,
    kpis: Kpis,
    warning: Option<IngestWarning>,
    tools: Vec<ToolRow>,
    tools_chart_json: String,
}

/// The KPI row.
struct Kpis {
    sessions: u64,
    tokens: String,
    /// e.g. `80% cache reads`; empty without input tokens.
    cache_read_share: String,
}

impl From<Consumption> for Kpis {
    fn from(c: Consumption) -> Self {
        Self {
            sessions: c.sessions,
            tokens: format::count(c.tokens.total()),
            cache_read_share: c
                .tokens
                .cache_read_share()
                .map(|share| format!("{:.0}% cache reads", share * 100.0))
                .unwrap_or_default(),
        }
    }
}

/// The ingest warning banner (shown when transcript lines were skipped).
struct IngestWarning {
    skipped_lines: u64,
    log_path: String,
}

impl IngestWarning {
    fn from_status(status: &IngestStatus, log_path: String) -> Option<Self> {
        (status.skipped_transcript_lines > 0).then_some(Self {
            skipped_lines: status.skipped_transcript_lines,
            log_path,
        })
    }
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
    let log_path = state.paths.log_file().display().to_string();
    let (tools, consumption, status) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let conn = db::open(&state.paths)?;
        Ok((
            tools::tool_ranking(&conn, &filter)?,
            consumption::consumption(&conn, &filter)?,
            ingest_status::ingest_status(&conn)?,
        ))
    })
    .await??;
    let tools: Vec<ToolRow> = tools.into_iter().map(ToolRow::from).collect();

    let page = OverviewPage {
        tools_chart_json: format::script_json(&tools)?,
        kpis: Kpis::from(consumption),
        warning: IngestWarning::from_status(&status, log_path),
        filters,
        tools,
    };
    Ok(Html(page.render()?))
}
