//! The overview (home) page.

mod skills_section;
mod time_section;

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;
use serde::Serialize;

use crate::db;
use crate::pricing::{Cost, PriceTable};
use crate::stats::consumption::{self, Consumption};
use crate::stats::cost;
use crate::stats::ingest_status::{self, IngestStatus};
use crate::stats::tools::{self, RankedCalls};
use crate::stats::{skills, subagents, time};
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;
use skills_section::{SkillRow, SubagentRow};
use time_section::TimeSection;

#[derive(Template)]
#[template(path = "pages/overview.html")]
struct OverviewPage {
    filters: FilterParams,
    kpis: Kpis,
    warning: Option<IngestWarning>,
    tools: Vec<ToolRow>,
    tools_chart_json: String,
    time: TimeSection,
    skills: Vec<SkillRow>,
    subagents: Vec<SubagentRow>,
}

/// The KPI row.
struct Kpis {
    sessions: u64,
    tokens: String,
    /// e.g. `80% cache reads`; empty without input tokens.
    cache_read_share: String,
    /// API-equivalent cost, e.g. `$12.34`, `$12.34+` (partial) or `unknown`.
    cost: String,
    /// What the cost covers, naming the unpriced models when partial.
    cost_note: String,
}

impl Kpis {
    fn new(c: Consumption, total_cost: &Cost) -> Self {
        let (cost, cost_note) = cost_kpi(total_cost);
        Self {
            cost,
            cost_note,
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

/// The cost tile's value and note. A partial sum is marked, and a cost made
/// only of unpriced models is `unknown`, never `$0`.
fn cost_kpi(cost: &Cost) -> (String, String) {
    if cost.is_complete() {
        return (cost.known.to_string(), "at API list prices".to_owned());
    }
    let models = cost
        .unknown_models
        .iter()
        .map(|m| {
            if m.is_empty() {
                "(no model)"
            } else {
                m.as_str()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let value = if cost.known.picos() == 0 {
        "unknown".to_owned()
    } else {
        format!("{}+", cost.known)
    };
    (value, format!("partial: no price for {models}"))
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
    let (tools, consumption, total_cost, status, time, skills, subagents) =
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let conn = db::open(&state.paths)?;
            Ok((
                tools::tool_ranking(&conn, &filter)?,
                consumption::consumption(&conn, &filter)?,
                cost::total_cost(&conn, &filter, PriceTable::builtin())?,
                ingest_status::ingest_status(&conn)?,
                TimeSection::build(
                    time::time_breakdown(&conn, &filter)?,
                    time::waiting_by_tool(&conn, &filter)?,
                )?,
                skills_section::skill_rows(skills::skill_ranking(&conn, &filter)?),
                skills_section::subagent_rows(subagents::subagent_ranking(&conn, &filter)?),
            ))
        })
        .await??;
    let tools: Vec<ToolRow> = tools.into_iter().map(ToolRow::from).collect();

    let page = OverviewPage {
        tools_chart_json: format::script_json(&tools)?,
        kpis: Kpis::new(consumption, &total_cost.cost),
        warning: IngestWarning::from_status(&status, log_path),
        filters,
        tools,
        time,
        skills,
        subagents,
    };
    Ok(Html(page.render()?))
}
