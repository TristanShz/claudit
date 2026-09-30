//! The session page (`/sessions/{id}`): header, KPIs, turn timeline, the
//! list of every turn (each opening its trace, see [`super::turn_trace`]),
//! the session's activities, tools, skills and subagents.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::Serialize;

use crate::db;
use crate::pricing::PriceTable;
use crate::stats::activities;
use crate::stats::session_detail::{self, SessionDetail};
use crate::stats::skills::SkillTrigger;
use crate::stats::time::{SegmentKind, TimeSplit, TurnTime};
use crate::stats::trace::{self, TurnRow};
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;
use crate::web::frame::Frame;
use crate::web::rows::{self, ActivityRow, RunRow, ToolRow};

#[derive(Template)]
#[template(path = "pages/session.html")]
struct SessionPage {
    frame: Frame,
    header: Header,
    /// Known only from its transcripts: no KPIs nor timeline, an
    /// explanation instead.
    imported: bool,
    /// Activity bars show calls (nothing in the session was hook-timed).
    activities_by_calls: bool,
    kpis: Vec<Kpi>,
    timeline_json: String,
    /// CSS height of the timeline, from its number of lanes.
    timeline_height: usize,
    /// Timed turns (timeline lanes).
    turns: usize,
    /// Every turn, timed or not.
    turn_count: u64,
    /// Every turn, oldest first.
    turn_list: Vec<TurnListRow>,
    activities: Vec<ActivityRow>,
    tools: Vec<ToolRow>,
    skills: Vec<SkillUse>,
    subagents: Vec<RunRow>,
}

struct Header {
    session_id: String,
    prompt: String,
    project: String,
    cwd: String,
    branch: String,
    model: String,
    version: String,
    /// e.g. `2026-03-02 09:00 → 09:30`.
    span: String,
    tokens: String,
    cache_read_share: String,
    cost: String,
    tool_calls: u64,
    active_time: String,
}

struct Kpi {
    /// `model`, `tool`, `waiting` or `subagent` (CSS hook).
    kind: &'static str,
    label: &'static str,
    value: String,
    sub: String,
}

/// A row of the turn list.
struct TurnListRow {
    prompt_id: String,
    number: usize,
    started: String,
    prompt: String,
    /// `typed`, `command` or `injected` (CSS hook).
    prompt_kind: &'static str,
    /// Hook-timed; `–` otherwise.
    duration: String,
    calls: u64,
    failed: u64,
    subagents: u64,
    tests: u64,
    failed_tests: u64,
    tokens: String,
    cost: String,
}

fn turn_list(turns: &[TurnRow]) -> Vec<TurnListRow> {
    turns
        .iter()
        .map(|t| TurnListRow {
            prompt_id: t.prompt_id.clone(),
            number: t.number,
            started: t.started_at.map(format::local_time_s).unwrap_or_default(),
            prompt: t
                .prompt
                .as_ref()
                .map(|p| format::truncate(&p.text, 200))
                .unwrap_or_default(),
            prompt_kind: match &t.prompt {
                Some(p) if p.kind.injected() => "injected",
                Some(p) if p.kind == crate::stats::prompt::PromptKind::Command => "command",
                _ => "typed",
            },
            duration: t.duration.map_or_else(|| "–".to_owned(), format::duration),
            calls: t.tool_calls,
            failed: t.failed_calls,
            subagents: t.subagent_runs,
            tests: t.test_runs,
            failed_tests: t.failed_test_runs,
            tokens: format::count(t.tokens.total()),
            cost: if t.tokens.total() == 0 {
                "–".to_owned()
            } else {
                format::cost(&t.cost)
            },
        })
        .collect()
}

/// A skill invocation of the session.
struct SkillUse {
    skill: String,
    pill: &'static str,
    pill_kind: &'static str,
    at: String,
    /// Invoked inside a subagent.
    in_subagent: bool,
}

/// The timeline chart's data: one lane per turn, segments in ms from the
/// turn's start.
#[derive(Serialize)]
struct Timeline {
    kinds: Vec<TimelineKind>,
    turns: Vec<TimelineTurn>,
}

#[derive(Serialize)]
struct TimelineKind {
    kind: &'static str,
    label: &'static str,
}

#[derive(Serialize)]
struct TimelineTurn {
    /// `#1`, `#2`, …
    label: String,
    prompt: String,
    started: String,
    duration_ms: i64,
    /// `[kind index, start ms, end ms]`, offsets from the turn's start.
    segments: Vec<(usize, i64, i64)>,
}

/// `rows` gives each turn's number and label (the turn list's).
fn timeline(turns: &[TurnTime], rows: &[TurnRow]) -> Timeline {
    let by_id: std::collections::HashMap<&str, &TurnRow> =
        rows.iter().map(|r| (r.prompt_id.as_str(), r)).collect();
    Timeline {
        kinds: SegmentKind::ALL
            .into_iter()
            .map(|kind| TimelineKind {
                kind: kind.name(),
                label: kind.label(),
            })
            .collect(),
        turns: turns
            .iter()
            .enumerate()
            .map(|(i, turn)| {
                let row = by_id.get(turn.prompt_id.as_str());
                TimelineTurn {
                    label: format!("#{}", row.map_or(i + 1, |r| r.number)),
                    prompt: format::truncate(
                        row.and_then(|r| r.prompt.as_ref())
                            .map_or("", |p| p.text.as_str()),
                        300,
                    ),
                    started: format::local_time(turn.start),
                    duration_ms: (turn.end - turn.start).num_milliseconds(),
                    segments: turn
                        .segments
                        .iter()
                        .map(|s| {
                            (
                                SegmentKind::ALL
                                    .iter()
                                    .position(|k| *k == s.kind)
                                    .expect("every kind is listed"),
                                (s.start - turn.start).num_milliseconds(),
                                (s.end - turn.start).num_milliseconds(),
                            )
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

fn kpis(detail: &SessionDetail) -> Vec<Kpi> {
    let time: &TimeSplit = &detail.time;
    let wall = time.wall().num_milliseconds();
    let share = |d: chrono::Duration| {
        if wall > 0 {
            format!(
                "{:.0}% of active time",
                d.num_milliseconds() as f64 * 100.0 / wall as f64
            )
        } else {
            String::new()
        }
    };
    let calls: u64 = detail.tools.iter().map(|t| t.stats.calls).sum();
    let failures: u64 = detail.tools.iter().map(|t| t.stats.failures).sum();
    let runs = detail.subagents.len();
    // Runs overlap and may lack hook timings: their own active time, summed.
    let timed_runs = detail
        .subagents
        .iter()
        .filter(|r| r.duration.is_some())
        .count();
    let run_time = detail
        .subagents
        .iter()
        .filter_map(|r| r.duration)
        .fold(chrono::Duration::zero(), |a, b| a + b);
    vec![
        Kpi {
            kind: "model",
            label: "Model",
            value: format::duration(time.model),
            sub: share(time.model),
        },
        Kpi {
            kind: "tool",
            label: "Tools",
            value: format::duration(time.tool),
            sub: format!(
                "{calls} call{}, {failures} failed · {}",
                if calls == 1 { "" } else { "s" },
                share(time.tool)
            ),
        },
        Kpi {
            kind: "waiting",
            label: "Waiting on you",
            value: format::duration(time.waiting),
            sub: share(time.waiting),
        },
        Kpi {
            kind: "subagent",
            label: "Waiting on subagents",
            value: format::duration(time.subagent),
            sub: format!(
                "{runs} run{}, {} active{} · {}",
                if runs == 1 { "" } else { "s" },
                format::duration(run_time),
                if timed_runs < runs {
                    format!(" ({timed_runs} timed)")
                } else {
                    String::new()
                },
                share(time.subagent)
            ),
        },
    ]
}

fn header(detail: &SessionDetail) -> Header {
    let cwd = detail.cwd.clone().unwrap_or_default();
    let span = match (detail.started_at, detail.last_activity_at) {
        (Some(start), Some(end)) => {
            let (start, end) = (format::local_time(start), format::local_time(end));
            // Same day: show only the end's time.
            match (start.split_once(' '), end.split_once(' ')) {
                (Some((d1, _)), Some((d2, t2))) if d1 == d2 => format!("{start} → {t2}"),
                _ => format!("{start} → {end}"),
            }
        }
        (Some(start), None) => format::local_time(start),
        _ => String::new(),
    };
    Header {
        session_id: detail.session_id.clone(),
        prompt: format::truncate(detail.first_prompt.as_deref().unwrap_or(""), 400),
        project: rows::project_name(&cwd),
        cwd,
        branch: detail.git_branch.clone().unwrap_or_default(),
        model: detail.model.clone().unwrap_or_default(),
        version: detail.version.clone().unwrap_or_default(),
        span,
        tokens: format::count(detail.tokens.total()),
        cache_read_share: detail
            .tokens
            .cache_read_share()
            .map(|s| format!("{:.0}% cache reads", s * 100.0))
            .unwrap_or_default(),
        cost: format::cost(&detail.cost),
        tool_calls: detail.tools.iter().map(|t| t.stats.calls).sum(),
        active_time: if detail.imported {
            "–".to_owned()
        } else {
            format::duration(detail.time.wall())
        },
    }
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(filters): Query<FilterParams>,
) -> Result<Response, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<SessionPage>> {
        let conn = db::open(&state.paths)?;
        let Some(detail) =
            session_detail::session_detail(&conn, &session_id, PriceTable::builtin())?
        else {
            return Ok(None);
        };
        let path = format!("/sessions/{}", detail.session_id);
        let frame = Frame::load(&conn, &state, filters, &path, false)?;
        let turns = detail.turns.len();
        let turn_rows = trace::session_turns(
            &conn,
            &detail.session_id,
            &frame.rules,
            PriceTable::builtin(),
        )?;
        let breakdown = activities::session_activities(&conn, &detail.session_id, &frame.rules)?;
        let activities = rows::activity_rows(&breakdown);
        Ok(Some(SessionPage {
            frame,
            header: header(&detail),
            imported: detail.imported,
            activities_by_calls: rows::activities_by_calls(&breakdown),
            kpis: kpis(&detail),
            timeline_json: format::script_json(&timeline(&detail.turns, &turn_rows))?,
            timeline_height: 60 + 46 * turns.max(1),
            turns,
            turn_count: detail.turn_count,
            turn_list: turn_list(&turn_rows),
            activities,
            skills: detail
                .skills
                .iter()
                .map(|s| {
                    let (pill, pill_kind) = rows::trigger_pill(
                        s.trigger == SkillTrigger::User,
                        s.trigger == SkillTrigger::Model,
                    );
                    SkillUse {
                        skill: s.skill.clone(),
                        pill,
                        pill_kind,
                        at: format::local_time(s.at),
                        in_subagent: s.agent_id.is_some(),
                    }
                })
                .collect(),
            tools: rows::tool_rows(detail.tools),
            subagents: rows::run_rows(detail.subagents),
        }))
    })
    .await??;
    match page {
        Some(page) => Ok(Html(page.render()?).into_response()),
        None => Ok((StatusCode::NOT_FOUND, "Unknown session").into_response()),
    }
}
