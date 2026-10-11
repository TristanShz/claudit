//! The team page: the period reports in the team folder, member by member
//! against the team median (see [`crate::team`]), and the form that sets
//! the folder.

use std::path::PathBuf;
use std::sync::Arc;

use askama::Template;
use axum::Form;
use axum::extract::{Query, State};
use axum::response::{Html, Redirect};
use serde::Deserialize;

use crate::db;
use crate::report::Period;
use crate::stats::time::SegmentKind;
use crate::team::{self, Comparison, Deviation, Row, Skipped, TeamSettings, Unit};
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;
use crate::web::frame::Frame;

/// Rows shown per item table (skills, tools, …).
const TOP_ITEMS: usize = 15;

#[derive(Template)]
#[template(path = "pages/team.html")]
struct TeamPage {
    frame: Frame,
    /// The folder, as saved (empty when none).
    folder: String,
    /// Why the folder could not be read.
    error: Option<String>,
    skipped: Vec<Skipped>,
    choices: Vec<PeriodOption>,
    comparison: Option<ComparisonView>,
}

struct PeriodOption {
    /// `FROM_TO`.
    value: String,
    label: String,
    selected: bool,
}

struct ComparisonView {
    members: Vec<MemberView>,
    metrics: Vec<RowView>,
    tables: Vec<TableView>,
}

struct MemberView {
    name: String,
    /// e.g. `118 recorded · 12 imported`.
    sessions: String,
    claudit_version: String,
    generated: String,
    /// The filters beyond the period, e.g. `project acme/api`; empty when
    /// none (a filtered report is not comparable with the others).
    filter: String,
    time: Vec<TimeBar>,
    /// Wall time of the timed turns, formatted; empty when none.
    time_total: String,
}

struct TimeBar {
    kind: &'static str,
    label: &'static str,
    /// Share of the wall time, in percent.
    pct: f64,
    share: String,
}

struct TableView {
    id: &'static str,
    title: &'static str,
    note: &'static str,
    rows: Vec<RowView>,
    /// Rows left out of `rows`.
    more: usize,
}

struct RowView {
    label: String,
    help: String,
    cells: Vec<CellView>,
    median: String,
}

struct CellView {
    text: String,
    /// `above`, `below` or empty (CSS hook).
    class: &'static str,
    /// e.g. `2.1× the team median`.
    title: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(in crate::web) struct TeamParams {
    /// `FROM_TO`; default: the period most members reported.
    period: String,
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TeamParams>,
) -> Result<Html<String>, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<TeamPage> {
        let conn = db::open(&state.paths)?;
        let frame = Frame::load(&conn, &state, FilterParams::default(), "/team", false)?;
        let settings = TeamSettings::load(&state.paths);
        let mut page = TeamPage {
            frame,
            folder: settings
                .dir
                .as_ref()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            error: None,
            skipped: Vec::new(),
            choices: Vec::new(),
            comparison: None,
        };
        let Some(dir) = settings.dir else {
            return Ok(page);
        };
        let loaded = match team::load_dir(&dir) {
            Ok(loaded) => loaded,
            Err(err) => {
                page.error = Some(format!("{err:#}"));
                return Ok(page);
            }
        };
        page.skipped = loaded.skipped;
        let choices = team::periods(&loaded.reports);
        let wanted = parse_period(&params.period);
        let Some(chosen) = wanted
            .filter(|p| choices.iter().any(|c| c.period == *p))
            .or_else(|| choices.first().map(|c| c.period))
        else {
            return Ok(page);
        };
        page.choices = choices
            .iter()
            .map(|c| PeriodOption {
                value: format!("{}_{}", c.period.from, c.period.to),
                label: format!(
                    "{} → {} · {} member{}",
                    c.period.from,
                    c.period.to,
                    c.members,
                    if c.members == 1 { "" } else { "s" }
                ),
                selected: c.period == chosen,
            })
            .collect();
        page.comparison = Some(view(&team::compare(&loaded.reports, chosen)));
        Ok(page)
    })
    .await??;
    Ok(Html(page.render()?))
}

#[derive(Deserialize)]
pub(in crate::web) struct FolderForm {
    #[serde(default)]
    dir: String,
}

/// Saves the team folder (an empty one forgets it), then shows the page.
pub(in crate::web) async fn set_folder(
    State(state): State<Arc<AppState>>,
    Form(form): Form<FolderForm>,
) -> Result<Redirect, WebError> {
    tokio::task::spawn_blocking(move || {
        let dir = form.dir.trim();
        TeamSettings {
            dir: (!dir.is_empty()).then(|| PathBuf::from(dir)),
        }
        .save(&state.paths)
    })
    .await??;
    Ok(Redirect::to("/team"))
}

fn parse_period(value: &str) -> Option<Period> {
    let (from, to) = value.split_once('_')?;
    let day = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
    Some(Period {
        from: day(from)?,
        to: day(to)?,
    })
}

fn view(c: &Comparison) -> ComparisonView {
    let members = c
        .reports
        .iter()
        .map(|r| {
            let t = r.time;
            let parts = [
                (SegmentKind::Model, t.model),
                (SegmentKind::Tool, t.tool),
                (SegmentKind::Waiting, t.waiting),
                (SegmentKind::Subagent, t.subagent),
                (SegmentKind::Background, t.background),
            ];
            let wall: u64 = parts.iter().map(|(_, ms)| ms).sum();
            let filter = [
                ("project", &r.filter.project),
                ("branch", &r.filter.branch),
                ("model", &r.filter.model),
            ]
            .into_iter()
            .filter_map(|(name, value)| value.as_ref().map(|v| format!("{name} {v}")))
            .collect::<Vec<_>>()
            .join(", ");
            MemberView {
                name: r.author.clone(),
                sessions: format!(
                    "{} recorded · {} imported",
                    r.coverage.recorded_sessions, r.coverage.imported_sessions
                ),
                claudit_version: r.claudit_version.clone(),
                generated: format::local_time(r.generated_at),
                filter,
                time: if wall == 0 {
                    Vec::new()
                } else {
                    parts
                        .into_iter()
                        .map(|(kind, ms)| {
                            let ratio = ms as f64 / wall as f64;
                            TimeBar {
                                kind: kind.name(),
                                label: kind.label(),
                                pct: ratio * 100.0,
                                share: format::percent(ratio),
                            }
                        })
                        .collect()
                },
                time_total: if wall == 0 {
                    String::new()
                } else {
                    format::duration_ms(wall)
                },
            }
        })
        .collect();
    let table = |id, title, note, rows: &[Row]| TableView {
        id,
        title,
        note,
        rows: rows.iter().take(TOP_ITEMS).map(row_view).collect(),
        more: rows.len().saturating_sub(TOP_ITEMS),
    };
    ComparisonView {
        members,
        metrics: c.metrics.iter().map(row_view).collect(),
        tables: vec![
            table(
                "team-skills",
                "Skills",
                "Invocations, typed or by Claude, per 100 turns.",
                &c.skills,
            ),
            table(
                "team-subagents",
                "Subagents",
                "Runs per 100 turns.",
                &c.subagents,
            ),
            table("team-tools", "Tools", "Calls per turn.", &c.tools),
            table(
                "team-activities",
                "Activities",
                "Share of each member's timed tool time.",
                &c.activities,
            ),
            table(
                "team-models",
                "Models",
                "Share of each member's cost.",
                &c.models,
            ),
        ],
    }
}

fn row_view(row: &Row) -> RowView {
    let cells = row
        .values
        .iter()
        .zip(&row.deviations)
        .map(|(value, deviation)| {
            let class = match deviation {
                Deviation::Above => "above",
                Deviation::Below => "below",
                Deviation::Typical => "",
            };
            let title = match (value, row.median) {
                (Some(v), Some(m)) if m > 0.0 && !class.is_empty() => {
                    format!("{:.1}× the team median", v / m)
                }
                (Some(v), Some(m)) if m == 0.0 && *v > 0.0 => {
                    "the rest of the team is at zero".to_owned()
                }
                _ => String::new(),
            };
            CellView {
                text: value.map_or_else(|| "–".to_owned(), |v| fmt(v, row.unit)),
                class,
                title,
            }
        })
        .collect();
    RowView {
        label: row.label.clone(),
        help: row.help.clone(),
        cells,
        median: row
            .median
            .map_or_else(|| "–".to_owned(), |m| fmt(m, row.unit)),
    }
}

/// A value as its unit reads; a small but non-zero value never shows as 0.
fn fmt(value: f64, unit: Unit) -> String {
    match unit {
        Unit::Number if value == 0.0 => "–".to_owned(),
        Unit::Number if value < 0.1 => "<0.1".to_owned(),
        Unit::Number if value >= 1000.0 => format::count(value.round() as u64),
        Unit::Number if value == value.round() => format!("{value:.0}"),
        Unit::Number => format!("{value:.1}"),
        Unit::Share if value == 0.0 => "–".to_owned(),
        Unit::Share if value < 0.001 => "<0.1 %".to_owned(),
        Unit::Share => format::percent((value * 1000.0).round() / 1000.0),
        Unit::Duration => format::duration_ms(value.round() as u64),
        Unit::Usd if value >= 100.0 => format!("${value:.0}"),
        Unit::Usd => format!("${value:.2}"),
    }
}
