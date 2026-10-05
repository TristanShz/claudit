//! The session page (`/sessions/{id}`): header, KPIs, turn timeline, the
//! list of every turn (each opening its trace, see [`super::turn_trace`]),
//! the session's activities, Bash commands, tools, skills and subagents.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::Serialize;

use crate::db;
use crate::pricing::PriceTable;
use crate::stats::activities;
use crate::stats::commands::{self, CommandSort};
use crate::stats::session_detail::{self, SessionDetail};
use crate::stats::skills::SkillTrigger;
use crate::stats::subagents::SubagentRun;
use crate::stats::time::{SegmentKind, TimeSplit, TurnTime};
use crate::stats::trace::{self, TurnRow};
use crate::web::AppState;
use crate::web::commands_table::{CommandsTable, SortParam, page_href};
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
    /// Timed turns (on the timeline's main lane).
    turns: usize,
    /// Most subagent runs alive at once (the timeline's tracks).
    parallel_runs: usize,
    /// Every turn, timed or not.
    turn_count: u64,
    /// Every turn, oldest first.
    turn_list: Vec<TurnListRow>,
    activities: Vec<ActivityRow>,
    /// Its Bash commands, sortable.
    commands: CommandsTable,
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

/// The session timeline's data, on the session's clock: the main thread's
/// turns on one lane, the subagent runs packed on tracks below it (a track
/// per run running alongside another). Times are ms from `origin_ms`.
#[derive(Serialize)]
struct Timeline {
    kinds: Vec<TimelineKind>,
    /// The earliest turn or run start, in Unix ms (the axis' clock).
    origin_ms: i64,
    turns: Vec<TimelineTurn>,
    runs: Vec<TimelineRun>,
    /// Subagent tracks (lanes under the main thread's).
    tracks: usize,
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
    /// Opens the turn in the turn list.
    prompt_id: String,
    prompt: String,
    started: String,
    /// UserPromptSubmit → Stop.
    duration_ms: i64,
    /// `[kind index, start ms, end ms]`: the turn tiled by kind.
    segments: Vec<(usize, i64, i64)>,
}

#[derive(Serialize)]
struct TimelineRun {
    track: usize,
    /// Its parent turn's, opened on click.
    prompt_id: String,
    agent_type: String,
    description: String,
    /// `subagent`'s kind index when the main thread waited on it,
    /// `background`'s otherwise.
    kind: usize,
    duration_ms: i64,
    tool_calls: u64,
    /// `[start ms, end ms]`: its hook-timed active spans.
    spans: Vec<(i64, i64)>,
}

fn kind_index(kind: SegmentKind) -> usize {
    SegmentKind::ALL
        .iter()
        .position(|k| *k == kind)
        .expect("every kind is listed")
}

/// `rows` gives each turn's number and label (the turn list's); `runs`
/// without hook-timed spans are left out.
fn timeline(turns: &[TurnTime], rows: &[TurnRow], runs: &[SubagentRun]) -> Timeline {
    let by_id: std::collections::HashMap<&str, &TurnRow> =
        rows.iter().map(|r| (r.prompt_id.as_str(), r)).collect();
    let runs: Vec<&SubagentRun> = runs.iter().filter(|r| !r.active.is_empty()).collect();
    let origin = turns
        .iter()
        .map(|t| t.start)
        .chain(runs.iter().map(|r| r.active[0].0))
        .min()
        .unwrap_or_default();
    let ms = |at: chrono::DateTime<chrono::Utc>| (at - origin).num_milliseconds();
    // Main-thread waits on a subagent, telling foreground runs.
    let waits: Vec<(i64, i64)> = turns
        .iter()
        .flat_map(|t| &t.segments)
        .filter(|s| s.kind == SegmentKind::Subagent)
        .map(|s| (ms(s.start), ms(s.end)))
        .collect();
    // Greedy packing by start: as many tracks as runs ever overlapped.
    let mut ordered: Vec<(i64, i64, &SubagentRun)> = runs
        .iter()
        .map(|r| (ms(r.active[0].0), ms(r.active[r.active.len() - 1].1), *r))
        .collect();
    ordered.sort_by_key(|(start, end, _)| (*start, *end));
    let mut track_ends: Vec<i64> = Vec::new();
    let mut timeline_runs = Vec::with_capacity(ordered.len());
    for (start, end, run) in ordered {
        let track = match track_ends.iter().position(|&e| e <= start) {
            Some(t) => t,
            None => {
                track_ends.push(end);
                track_ends.len() - 1
            }
        };
        track_ends[track] = end;
        let spans: Vec<(i64, i64)> = run.active.iter().map(|(a, b)| (ms(*a), ms(*b))).collect();
        // A background launch's Agent call overlaps its run's first
        // instants: foreground only when waited on most of its time.
        let active: i64 = spans.iter().map(|(a, b)| b - a).sum();
        let waited: i64 = spans
            .iter()
            .flat_map(|(a, b)| {
                waits
                    .iter()
                    .map(move |(wa, wb)| (*wb.min(b) - *wa.max(a)).max(0))
            })
            .sum();
        let foreground = waited * 2 >= active && waited > 0;
        timeline_runs.push(TimelineRun {
            track,
            prompt_id: run.prompt_id.clone().unwrap_or_default(),
            agent_type: run.agent_type.clone(),
            description: run.description.clone().unwrap_or_default(),
            kind: kind_index(if foreground {
                SegmentKind::Subagent
            } else {
                SegmentKind::Background
            }),
            duration_ms: run.duration.map_or(0, |d| d.num_milliseconds()),
            tool_calls: run.tool_calls,
            spans,
        });
    }
    Timeline {
        kinds: SegmentKind::ALL
            .into_iter()
            .map(|kind| TimelineKind {
                kind: kind.name(),
                label: kind.label(),
            })
            .collect(),
        origin_ms: origin.timestamp_millis(),
        turns: turns
            .iter()
            .enumerate()
            .map(|(i, turn)| {
                let row = by_id.get(turn.prompt_id.as_str());
                TimelineTurn {
                    label: format!("#{}", row.map_or(i + 1, |r| r.number)),
                    prompt_id: turn.prompt_id.clone(),
                    prompt: format::truncate(
                        row.and_then(|r| r.prompt.as_ref())
                            .map_or("", |p| p.text.as_str()),
                        300,
                    ),
                    started: format::local_time_s(turn.start),
                    duration_ms: (turn.end - turn.start).num_milliseconds(),
                    // Background stretches are the runs' lanes.
                    segments: turn
                        .segments
                        .iter()
                        .filter(|s| s.kind != SegmentKind::Background)
                        .map(|s| (kind_index(s.kind), ms(s.start), ms(s.end)))
                        .collect(),
                }
            })
            .collect(),
        tracks: track_ends.len(),
        runs: timeline_runs,
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
        Kpi {
            kind: "background",
            label: "Background subagents",
            value: format::duration(time.background),
            sub: format!(
                "between turns, main thread idle · {}",
                share(time.background)
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
    Query(sort): Query<SortParam>,
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
        let timeline = timeline(&detail.turns, &turn_rows, &detail.subagents);
        let breakdown = activities::session_activities(&conn, &detail.session_id, &frame.rules)?;
        let activities = rows::activity_rows(&breakdown);
        let sort = sort.sort_or(CommandSort::Total);
        let ranking = commands::session_commands(&conn, &detail.session_id, &frame.rules, sort)?;
        let link = |name: &str| {
            page_href(
                &path,
                &frame.query,
                &[("sort", name)],
                "#session-commands-section",
            )
        };
        let commands = CommandsTable::new(&ranking, usize::MAX, Some(sort), Some(&link));
        Ok(Some(SessionPage {
            frame,
            header: header(&detail),
            imported: detail.imported,
            activities_by_calls: rows::activities_by_calls(&breakdown),
            kpis: kpis(&detail),
            timeline_json: format::script_json(&timeline)?,
            timeline_height: 70 + 26 * (1 + timeline.tracks),
            parallel_runs: timeline.tracks,
            turns,
            turn_count: detail.turn_count,
            turn_list: turn_list(&turn_rows),
            activities,
            commands,
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
