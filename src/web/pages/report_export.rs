//! The period report (`/report?from=&to=&project=&branch=&model=&author=&prompts=on`),
//! as a JSON file download: what `claudit report` writes (see
//! [`crate::report`]), over the filter bar's filters.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::NaiveDate;
use serde::Deserialize;

use crate::activities::ActivityRules;
use crate::db;
use crate::pricing::PriceTable;
use crate::report::{self, Period, ReportOptions};
use crate::web::AppState;
use crate::web::error::WebError;

/// Raw query parameters; empty strings mean "not given".
#[derive(Deserialize, Default)]
#[serde(default)]
pub(in crate::web) struct ReportParams {
    /// First day included (`YYYY-MM-DD`, UTC); default: the
    /// [`report::DEFAULT_DAYS`] days ending with `to`.
    from: String,
    /// Last day included; default: today.
    to: String,
    project: String,
    branch: String,
    model: String,
    /// Default: `git config user.name`.
    author: String,
    /// `on` adds the typed prompts.
    prompts: String,
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ReportParams>,
) -> Result<Response, WebError> {
    let options = match options(&params) {
        Ok(options) => options,
        Err(err) => return Ok((StatusCode::BAD_REQUEST, format!("{err:#}")).into_response()),
    };
    let built = tokio::task::spawn_blocking(move || -> anyhow::Result<report::Report> {
        let conn = db::open(&state.paths)?;
        let rules = ActivityRules::load(&state.paths).rules;
        report::build(
            &conn,
            &options,
            &rules,
            PriceTable::builtin(),
            chrono::Utc::now(),
            &report::git_project_name,
        )
    })
    .await??;
    let mut json = serde_json::to_string_pretty(&built)?;
    json.push('\n');
    let disposition = format!("attachment; filename=\"{}\"", report::file_name(&built));
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "application/json; charset=utf-8".to_owned(),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        json,
    )
        .into_response())
}

/// The report the parameters ask for, or why they are invalid.
fn options(params: &ReportParams) -> anyhow::Result<ReportOptions> {
    let non_empty = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
    let day = |s: &str, name: &str| -> anyhow::Result<Option<NaiveDate>> {
        non_empty(s)
            .map(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d"))
            .transpose()
            .map_err(|_| anyhow::anyhow!("`{name}` is not a YYYY-MM-DD day"))
    };
    let from = day(&params.from, "from")?;
    let today = chrono::Utc::now().date_naive();
    let days = from.is_none().then_some(report::DEFAULT_DAYS);
    Ok(ReportOptions {
        author: non_empty(&params.author)
            .or_else(report::git_user_name)
            .ok_or_else(|| {
                anyhow::anyhow!("no author: fill it in, or set `git config user.name`")
            })?,
        period: Period::resolve(from, day(&params.to, "to")?, days, today)?,
        project: non_empty(&params.project),
        branch: non_empty(&params.branch),
        model: non_empty(&params.model),
        include_prompts: params.prompts == "on",
    })
}
