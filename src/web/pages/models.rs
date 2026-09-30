//! All models, with the main thread / subagents split.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;

use crate::db;
use crate::pricing::PriceTable;
use crate::stats::models;
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;
use crate::web::frame::Frame;
use crate::web::rows::{self, ModelRow};

#[derive(Template)]
#[template(path = "pages/models.html")]
struct ModelsPage {
    frame: Frame,
    models: Vec<ModelRow>,
    total_tokens: String,
    total_cost: String,
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<FilterParams>,
) -> Result<Html<String>, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<ModelsPage> {
        let conn = db::open(&state.paths)?;
        let filter = filters.to_filter();
        let frame = Frame::load(&conn, &state, filters, "/models", true)?;
        let report = models::model_usage(&conn, &filter, PriceTable::builtin())?;
        Ok(ModelsPage {
            frame,
            models: rows::model_rows(&report),
            total_tokens: format::count(report.total_tokens.total()),
            total_cost: format::cost(&report.total_cost),
        })
    })
    .await??;
    Ok(Html(page.render()?))
}
