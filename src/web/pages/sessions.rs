//! All sessions.

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;

use crate::db;
use crate::pricing::{Cost, PriceTable};
use crate::stats::{cost, sessions};
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::frame::Frame;
use crate::web::rows::{self, SessionRow};

#[derive(Template)]
#[template(path = "pages/sessions.html")]
struct SessionsPage {
    frame: Frame,
    sessions: Vec<SessionRow>,
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<FilterParams>,
) -> Result<Html<String>, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<SessionsPage> {
        let conn = db::open(&state.paths)?;
        let filter = filters.to_filter();
        let frame = Frame::load(&conn, &state, filters, "/sessions", true)?;
        let costs: HashMap<String, Cost> =
            cost::cost_by_session(&conn, &filter, PriceTable::builtin())?
                .into_iter()
                .map(|line| (line.key, line.cost))
                .collect();
        Ok(SessionsPage {
            frame,
            sessions: rows::session_rows(sessions::session_list(&conn, &filter)?, &costs),
        })
    })
    .await??;
    Ok(Html(page.render()?))
}
