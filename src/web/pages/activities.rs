//! What Claude's tools spend time on: every activity, its top commands and
//! tools, and a daily stacked chart.

use std::collections::BTreeMap;
use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;
use serde::Serialize;

use crate::db;
use crate::stats::activities::{self, ActivityBreakdown};
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;
use crate::web::frame::Frame;
use crate::web::rows::{self, ActivityRow};

#[derive(Template)]
#[template(path = "pages/activities.html")]
struct ActivitiesPage {
    frame: Frame,
    activities: Vec<ActivityRow>,
    activities_total: String,
    rules_version: String,
    chart_json: String,
}

/// The daily chart: one stacked series per activity (in ranking order),
/// one value (ms) per day.
#[derive(Serialize)]
struct DailyChart {
    days: Vec<String>,
    series: Vec<DailySeries>,
}

#[derive(Serialize)]
struct DailySeries {
    name: String,
    color: usize,
    ms: Vec<u64>,
    calls: Vec<u64>,
}

fn daily_chart(breakdown: &ActivityBreakdown) -> DailyChart {
    let days: Vec<String> = breakdown
        .by_day
        .iter()
        .map(|d| d.day.format("%Y-%m-%d").to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let index: BTreeMap<&str, usize> = days
        .iter()
        .enumerate()
        .map(|(i, d)| (d.as_str(), i))
        .collect();
    let mut series: Vec<DailySeries> = breakdown
        .activities
        .iter()
        .map(|a| DailySeries {
            name: a.activity.clone(),
            color: rows::activity_color(&a.activity),
            ms: vec![0; days.len()],
            calls: vec![0; days.len()],
        })
        .collect();
    for d in &breakdown.by_day {
        let day = d.day.format("%Y-%m-%d").to_string();
        if let (Some(s), Some(&i)) = (
            series.iter_mut().find(|s| s.name == d.activity),
            index.get(day.as_str()),
        ) {
            s.ms[i] = d.duration_ms;
            s.calls[i] = d.calls;
        }
    }
    DailyChart { days, series }
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<FilterParams>,
) -> Result<Html<String>, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<ActivitiesPage> {
        let conn = db::open(&state.paths)?;
        let filter = filters.to_filter();
        let frame = Frame::load(&conn, &state, filters, "/activities", true)?;
        let breakdown = activities::activity_breakdown(&conn, &filter, &frame.rules)?;
        Ok(ActivitiesPage {
            activities: rows::activity_rows(&breakdown),
            activities_total: format::duration_ms(breakdown.total_duration_ms),
            rules_version: format!("rules {}", frame.rules.version),
            chart_json: format::script_json(&daily_chart(&breakdown))?,
            frame,
        })
    })
    .await??;
    Ok(Html(page.render()?))
}
