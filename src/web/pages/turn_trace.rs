//! A turn's trace (`/sessions/{id}/turns/{prompt_id}`): an HTML fragment
//! the session page loads with htmx when a turn of its list is opened, so a
//! session of thousands of calls only ships the turns looked at. The chart
//! (swimlanes of calls over time), its activity filter and the call log are
//! drawn by `assets/claudit.js` from the JSON it embeds.

use std::collections::BTreeMap;
use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::activities::ActivityRules;
use crate::db;
use crate::pricing::PriceTable;
use crate::stats::trace::{self, LaneKind, TurnTrace};
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::format;
use crate::web::rows;

#[derive(Template)]
#[template(path = "partials/turn_trace.html")]
struct TracePartial {
    /// DOM id suffix (the prompt id).
    key: String,
    json: String,
    calls: usize,
    subagents: usize,
    /// e.g. `52 min 10 s`: what the chart spans.
    span: String,
    /// Some calls are known only from the transcript (drawn as ticks).
    untimed_calls: usize,
    /// The turn itself was not timed by the hooks.
    untimed_turn: bool,
    chips: Vec<Chip>,
    /// Initial chart height (the script sizes it to its lanes).
    height: usize,
}

/// An activity filter chip.
struct Chip {
    name: String,
    color: usize,
    calls: usize,
}

/// What the trace chart draws; times are ms from the trace's start.
#[derive(Serialize)]
struct TraceJson {
    /// The trace's start, ms since the epoch (tooltips show clock times).
    origin_ms: i64,
    span_ms: i64,
    /// The turn's `UserPromptSubmit` and `Stop`, when the hooks saw them.
    turn_start_ms: Option<i64>,
    turn_end_ms: Option<i64>,
    lanes: Vec<LaneJson>,
    /// Activity names and palette slots, indexed by `CallJson::a`.
    activities: Vec<(String, usize)>,
    calls: Vec<CallJson>,
}

#[derive(Serialize)]
struct LaneJson {
    label: String,
    detail: String,
    spans: Vec<(i64, i64)>,
}

/// One call, with short keys: a turn can hold thousands.
#[derive(Serialize)]
struct CallJson {
    /// Lane.
    l: usize,
    /// Activity index.
    a: usize,
    /// Tool.
    t: String,
    /// Summary.
    s: String,
    /// Launched.
    at: i64,
    /// Execution start and end (hook-timed only).
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<(i64, i64)>,
    /// Permission wait, ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    w: Option<i64>,
    /// `ok`, `failed` or `open` (no result known).
    st: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    e: Option<String>,
    /// Lane of the subagent it launched.
    #[serde(skip_serializing_if = "Option::is_none")]
    sp: Option<usize>,
}

fn partial(trace: &TurnTrace) -> anyhow::Result<TracePartial> {
    let ms = |at: DateTime<Utc>| (at - trace.start).num_milliseconds();
    let mut activity_index: BTreeMap<&str, usize> = BTreeMap::new();
    for call in &trace.calls {
        let next = activity_index.len();
        activity_index.entry(call.activity.as_str()).or_insert(next);
    }
    let mut activities: Vec<(String, usize)> = vec![Default::default(); activity_index.len()];
    for (name, &i) in &activity_index {
        activities[i] = ((*name).to_owned(), rows::activity_color(name));
    }
    let mut counts = vec![0usize; activities.len()];
    let calls: Vec<CallJson> = trace
        .calls
        .iter()
        .map(|c| {
            let a = activity_index[c.activity.as_str()];
            counts[a] += 1;
            CallJson {
                l: c.lane,
                a,
                t: c.tool_name.clone(),
                s: c.summary.clone(),
                at: ms(c.launched_at),
                x: c.exec_start
                    .zip(c.exec_end)
                    .map(|(x0, x1)| (ms(x0), ms(x1))),
                w: c.wait.map(|w| w.num_milliseconds()),
                st: match c.success {
                    Some(true) => "ok",
                    Some(false) => "failed",
                    None => "open",
                },
                e: c.error.as_deref().map(|e| format::truncate(e, 300)),
                sp: c.spawned_lane,
            }
        })
        .collect();
    let lanes: Vec<LaneJson> = trace
        .lanes
        .iter()
        .map(|lane| {
            let (label, detail) = match &lane.kind {
                LaneKind::Main => ("Main thread".to_owned(), String::new()),
                LaneKind::Subagent {
                    agent_id,
                    agent_type,
                    model,
                    description,
                } => {
                    let kind = agent_type
                        .clone()
                        .unwrap_or_else(|| format!("agent {}", &agent_id[..agent_id.len().min(8)]));
                    let label = match description {
                        Some(d) => format!("{kind} · {d}"),
                        None => kind,
                    };
                    (label, model.clone().unwrap_or_default())
                }
            };
            LaneJson {
                label,
                detail,
                spans: lane.spans.iter().map(|(a, b)| (ms(*a), ms(*b))).collect(),
            }
        })
        .collect();
    let (turn_start_ms, turn_end_ms) = match trace.lanes.first().and_then(|l| l.spans.first()) {
        Some((a, b)) => (Some(ms(*a)), Some(ms(*b))),
        None => (trace.turn.started_at.map(ms), None),
    };
    let mut chips: Vec<Chip> = activities
        .iter()
        .zip(&counts)
        .map(|((name, color), &calls)| Chip {
            name: name.clone(),
            color: *color,
            calls,
        })
        .collect();
    chips.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));
    let json = TraceJson {
        origin_ms: trace.start.timestamp_millis(),
        span_ms: ms(trace.end).max(1),
        turn_start_ms,
        turn_end_ms,
        lanes,
        activities,
        calls,
    };
    Ok(TracePartial {
        key: trace.turn.prompt_id.clone(),
        calls: trace.calls.len(),
        subagents: trace.lanes.len().saturating_sub(1),
        span: format::duration(trace.end - trace.start),
        untimed_calls: trace
            .calls
            .iter()
            .filter(|c| c.exec_start.is_none())
            .count(),
        untimed_turn: trace.turn.duration.is_none(),
        chips,
        height: 90 + 44 * trace.lanes.len().max(1),
        json: format::script_json(&json)?,
    })
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Path((session_id, prompt_id)): Path<(String, String)>,
) -> Result<Response, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<TracePartial>> {
        let conn = db::open(&state.paths)?;
        let rules = ActivityRules::load(&state.paths).rules;
        let Some(trace) = trace::turn_trace(
            &conn,
            &session_id,
            &prompt_id,
            &rules,
            PriceTable::builtin(),
        )?
        else {
            return Ok(None);
        };
        Ok(Some(partial(&trace)?))
    })
    .await??;
    match page {
        Some(page) => Ok(Html(page.render()?).into_response()),
        None => Ok((StatusCode::NOT_FOUND, "Unknown turn").into_response()),
    }
}
