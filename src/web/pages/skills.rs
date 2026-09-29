//! All skills.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;

use crate::db;
use crate::stats::skills;
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::frame::Frame;
use crate::web::rows::{self, SkillRow};

#[derive(Template)]
#[template(path = "pages/skills.html")]
struct SkillsPage {
    frame: Frame,
    skills: Vec<SkillRow>,
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<FilterParams>,
) -> Result<Html<String>, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<SkillsPage> {
        let conn = db::open(&state.paths)?;
        let filter = filters.to_filter();
        let frame = Frame::load(&conn, &state, filters, "/skills", true)?;
        Ok(SkillsPage {
            frame,
            skills: rows::skill_rows(skills::skill_ranking(&conn, &filter)?),
        })
    })
    .await??;
    Ok(Html(page.render()?))
}
