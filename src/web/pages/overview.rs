//! The overview (home) page: KPIs, where the time goes, the top tools,
//! skills and subagents, and the most recent sessions.

mod time_section;

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;

use crate::db;
use crate::pricing::{Cost, PriceTable};
use crate::stats::consumption::{self, Consumption};
use crate::stats::time::TimeBreakdown;
use crate::stats::tools::RankedCalls;
use crate::stats::{cost, sessions, skills, subagents, time, tools};
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;
use crate::web::frame::Frame;
use crate::web::rows::{self, SessionRow, SkillRow, SubagentRow, ToolRow};
use time_section::TimeSection;

/// Rows shown per ranked list.
const TOP: usize = 8;
/// Sessions shown in the table.
const RECENT_SESSIONS: usize = 20;

#[derive(Template)]
#[template(path = "pages/overview.html")]
struct OverviewPage {
    frame: Frame,
    kpis: Kpis,
    time: TimeSection,
    tools: Vec<ToolRow>,
    skills: Vec<SkillRow>,
    subagents: Vec<SubagentRow>,
    sessions: Vec<SessionRow>,
    more_sessions: usize,
}

/// The KPI row.
struct Kpis {
    sessions: u64,
    turns: u64,
    active_time: String,
    tool_calls: String,
    /// e.g. `3.2 % failed`.
    failure_rate: String,
    tokens: String,
    /// e.g. `80% cache reads`; empty without input tokens.
    cache_read_share: String,
    /// API-equivalent cost, e.g. `$12.34`, `$12.34+` (partial) or `unknown`.
    cost: String,
    /// What the cost covers, naming the unpriced models when partial.
    cost_note: String,
}

impl Kpis {
    fn new(
        c: Consumption,
        breakdown: &TimeBreakdown,
        tools: &[RankedCalls],
        total_cost: &Cost,
    ) -> Self {
        let calls: u64 = tools.iter().map(|t| t.stats.calls).sum();
        let failures: u64 = tools.iter().map(|t| t.stats.failures).sum();
        Self {
            sessions: c.sessions,
            turns: breakdown.turns,
            active_time: format::duration(breakdown.total.wall()),
            tool_calls: format::count(calls),
            failure_rate: if calls > 0 {
                format!("{} failed", format::percent(failures as f64 / calls as f64))
            } else {
                String::new()
            },
            tokens: format::count(c.tokens.total()),
            cache_read_share: c
                .tokens
                .cache_read_share()
                .map(|share| format!("{:.0}% cache reads", share * 100.0))
                .unwrap_or_default(),
            cost: format::cost(total_cost),
            cost_note: cost_note(total_cost),
        }
    }
}

/// What the cost tile covers, naming the unpriced models when partial.
fn cost_note(cost: &Cost) -> String {
    if cost.is_complete() {
        return "at API list prices".to_owned();
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
    format!("partial: no price for {models}")
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<FilterParams>,
) -> Result<Html<String>, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<OverviewPage> {
        let conn = db::open(&state.paths)?;
        let filter = filters.to_filter();
        let prices = PriceTable::builtin();
        let frame = Frame::load(&conn, &state, filters, "/", true)?;

        let tool_ranking = tools::tool_ranking(&conn, &filter)?;
        let breakdown = time::time_breakdown(&conn, &filter)?;
        let kpis = Kpis::new(
            consumption::consumption(&conn, &filter)?,
            &breakdown,
            &tool_ranking,
            &cost::total_cost(&conn, &filter, prices)?.cost,
        );
        let time = TimeSection::build(breakdown, time::waiting_by_tool(&conn, &filter)?)?;

        let costs: HashMap<String, Cost> = cost::cost_by_session(&conn, &filter, prices)?
            .into_iter()
            .map(|line| (line.key, line.cost))
            .collect();
        let all_sessions = sessions::session_list(&conn, &filter)?;
        let more_sessions = all_sessions.len().saturating_sub(RECENT_SESSIONS);
        let recent = all_sessions.into_iter().take(RECENT_SESSIONS).collect();

        let mut tools = rows::tool_rows(tool_ranking);
        let mut skills = rows::skill_rows(skills::skill_ranking(&conn, &filter)?);
        let mut subagents = rows::subagent_rows(subagents::subagent_ranking(&conn, &filter)?);
        tools.truncate(TOP);
        skills.truncate(TOP);
        subagents.truncate(TOP);

        Ok(OverviewPage {
            frame,
            kpis,
            time,
            tools,
            skills,
            subagents,
            sessions: rows::session_rows(recent, &costs),
            more_sessions,
        })
    })
    .await??;
    Ok(Html(page.render()?))
}
