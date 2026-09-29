//! All subagent types, and every run.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;

use crate::db;
use crate::stats::subagents;
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::frame::Frame;
use crate::web::rows::{self, RunRow, SubagentRow};

#[derive(Template)]
#[template(path = "pages/subagents.html")]
struct SubagentsPage {
    frame: Frame,
    subagents: Vec<SubagentRow>,
    runs: Vec<RunRow>,
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<FilterParams>,
) -> Result<Html<String>, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<SubagentsPage> {
        let conn = db::open(&state.paths)?;
        let filter = filters.to_filter();
        let frame = Frame::load(&conn, &state, filters, "/subagents", true)?;
        let mut runs = subagents::subagent_runs(&conn, &filter)?;
        runs.reverse(); // most recent first
        Ok(SubagentsPage {
            frame,
            subagents: rows::subagent_rows(subagents::subagent_ranking(&conn, &filter)?),
            runs: rows::run_rows(runs),
        })
    })
    .await??;
    Ok(Html(page.render()?))
}
