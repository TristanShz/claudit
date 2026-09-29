//! Where the time goes: each main-thread turn's wall time split into model,
//! tool, waiting and subagent time.
//!
//! Per turn:
//! - **wall** = Stop receive time − UserPromptSubmit receive time (falling
//!   back to the turn's transcript span when a hook is missing);
//! - **subagent** = the union of the execution intervals
//!   `[post − duration_ms, post]` of the main thread's `Agent` / `Task`
//!   calls (the subagent's own tool calls, which carry an `agent_id`, are
//!   never main-thread time);
//! - **tool** = the union of the other main-thread calls' execution
//!   intervals, minus subagent time; parallel calls overlap instead of
//!   adding up;
//! - **waiting** = the union of the calls' waiting intervals
//!   `[pre, post − duration_ms]` (PreToolUse until execution starts: a
//!   permission prompt), minus tool and subagent time;
//! - **model** = everything else in the turn.
//!
//! Every interval is clipped to the turn, and each instant of the turn is
//! assigned to exactly one component (subagent > tool > waiting > model),
//! so the four components always sum to the wall time exactly. The same
//! partition is exposed as positioned [`Segment`]s per turn, for timelines.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rusqlite::types::Value;
use rusqlite::{Connection, params, params_from_iter};
use serde::Serialize;

use super::{Filter, FilterColumns};
use crate::clock;

/// What a stretch of a turn was spent on. Declared in increasing priority:
/// an instant covered by several kinds belongs to the greatest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SegmentKind {
    /// The model generating (and everything not otherwise explained).
    Model,
    /// A tool call waiting to start: the user answering a permission prompt.
    Waiting,
    /// Main-thread tools executing.
    Tool,
    /// A subagent running (the main thread's Agent/Task tool executing).
    Subagent,
}

/// A stretch of a turn, positioned in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub kind: SegmentKind,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// A wall time split into its four components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSplit {
    pub model: Duration,
    pub tool: Duration,
    pub waiting: Duration,
    pub subagent: Duration,
}

impl Default for TimeSplit {
    fn default() -> Self {
        Self {
            model: Duration::zero(),
            tool: Duration::zero(),
            waiting: Duration::zero(),
            subagent: Duration::zero(),
        }
    }
}

impl TimeSplit {
    /// The sum of the components.
    pub fn wall(&self) -> Duration {
        self.model + self.tool + self.waiting + self.subagent
    }

    fn add(&mut self, other: &TimeSplit) {
        self.model += other.model;
        self.tool += other.tool;
        self.waiting += other.waiting;
        self.subagent += other.subagent;
    }

    fn component(&mut self, kind: SegmentKind) -> &mut Duration {
        match kind {
            SegmentKind::Model => &mut self.model,
            SegmentKind::Tool => &mut self.tool,
            SegmentKind::Waiting => &mut self.waiting,
            SegmentKind::Subagent => &mut self.subagent,
        }
    }
}

/// One main-thread turn's time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnTime {
    pub session_id: String,
    pub prompt_id: String,
    pub prompt_text: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub split: TimeSplit,
    /// The turn from `start` to `end`, tiled without gaps or overlaps;
    /// adjacent segments have different kinds.
    pub segments: Vec<Segment>,
}

/// One UTC day's time (turns counted on the day they started).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayTime {
    pub day: NaiveDate,
    pub split: TimeSplit,
}

/// The time breakdown over the filtered turns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimeBreakdown {
    /// Turns with both a start and an end.
    pub turns: u64,
    pub total: TimeSplit,
    /// Days with at least one turn, oldest first.
    pub by_day: Vec<DayTime>,
}

/// Waiting on the user, for one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolWaiting {
    pub tool_name: String,
    /// Sum over the tool's calls of PreToolUse → execution start. Calls
    /// waiting at the same time each count, and subagents' calls are
    /// included (a subagent's permission prompt waits on the user too).
    pub waiting: Duration,
    /// Calls that waited at all.
    pub calls_waited: u64,
    /// PermissionRequest events for the tool.
    pub permission_requests: u64,
}

/// Tool names whose execution is a subagent run.
const SUBAGENT_TOOLS: [&str; 2] = ["Agent", "Task"];

/// Turn start and end, preferring hook times over transcript times.
const TURN_START: &str = "COALESCE(t.submit_at_us, t.start_at_us)";
const TURN_END: &str = "COALESCE(t.stop_at_us, t.end_at_us)";

/// Filter columns of `turns t LEFT JOIN sessions s LEFT JOIN api_messages m`.
pub(super) const TURN_COLUMNS: FilterColumns = FilterColumns {
    time_us: TURN_START,
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some("m.model"),
};

/// Filter columns of `tool_calls tc LEFT JOIN sessions s`; a call's model
/// is the first model of its turn (or subagent).
const TOOL_CALL_COLUMNS: FilterColumns = FilterColumns {
    time_us: "tc.post_at_us",
    cwd: Some("COALESCE(tc.cwd, s.cwd)"),
    branch: Some("s.git_branch"),
    model: Some(
        "(SELECT m.model FROM api_messages m
          WHERE m.session_id = tc.session_id AND m.prompt_id = tc.prompt_id
            AND m.agent_id IS tc.agent_id
          ORDER BY m.at_us LIMIT 1)",
    ),
};

const PERMISSION_COLUMNS: FilterColumns = FilterColumns {
    time_us: "pr.at_us",
    cwd: Some("COALESCE(pr.cwd, s.cwd)"),
    branch: Some("s.git_branch"),
    model: Some(
        "(SELECT m.model FROM api_messages m
          WHERE m.session_id = pr.session_id AND m.prompt_id = pr.prompt_id
            AND m.agent_id IS pr.agent_id
          ORDER BY m.at_us LIMIT 1)",
    ),
};

/// Every filtered main-thread turn with an end, oldest first.
pub fn turn_times(conn: &Connection, filter: &Filter) -> Result<Vec<TurnTime>> {
    let where_ = filter.sql(&TURN_COLUMNS)?;
    load_turns(conn, &where_.clause, where_.params)
}

/// Every turn of one session, oldest first (the session timeline).
pub fn session_turn_times(conn: &Connection, session_id: &str) -> Result<Vec<TurnTime>> {
    load_turns(
        conn,
        "t.session_id = ?",
        vec![Value::Text(session_id.to_owned())],
    )
}

/// The split summed over the filtered turns, overall and per day.
pub fn time_breakdown(conn: &Connection, filter: &Filter) -> Result<TimeBreakdown> {
    let turns = turn_times(conn, filter)?;
    let mut total = TimeSplit::default();
    let mut days: BTreeMap<NaiveDate, TimeSplit> = BTreeMap::new();
    for turn in &turns {
        total.add(&turn.split);
        days.entry(turn.start.date_naive())
            .or_default()
            .add(&turn.split);
    }
    Ok(TimeBreakdown {
        turns: turns.len() as u64,
        total,
        by_day: days
            .into_iter()
            .map(|(day, split)| DayTime { day, split })
            .collect(),
    })
}

/// Tools that made the user wait, most waiting first.
pub fn waiting_by_tool(conn: &Connection, filter: &Filter) -> Result<Vec<ToolWaiting>> {
    let mut by_tool: BTreeMap<String, ToolWaiting> = BTreeMap::new();
    fn entry(map: &mut BTreeMap<String, ToolWaiting>, name: String) -> &mut ToolWaiting {
        map.entry(name.clone()).or_insert_with(|| ToolWaiting {
            tool_name: name,
            waiting: Duration::zero(),
            calls_waited: 0,
            permission_requests: 0,
        })
    }

    let where_ = filter.sql(&TOOL_CALL_COLUMNS)?;
    let sql = format!(
        "SELECT tc.tool_name,
                SUM(MAX(0, tc.post_at_us - tc.duration_ms * 1000 - tc.pre_at_us)),
                SUM(tc.post_at_us - tc.duration_ms * 1000 > tc.pre_at_us)
         FROM tool_calls tc LEFT JOIN sessions s ON s.session_id = tc.session_id
         WHERE tc.pre_at_us IS NOT NULL AND tc.post_at_us IS NOT NULL
           AND tc.duration_ms IS NOT NULL AND {}
         GROUP BY tc.tool_name",
        where_.clause
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(where_.params), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (name, waiting_us, waited) = row?;
        let tool = entry(&mut by_tool, name);
        tool.waiting = Duration::microseconds(waiting_us);
        tool.calls_waited = waited as u64;
    }

    let where_ = filter.sql(&PERMISSION_COLUMNS)?;
    let sql = format!(
        "SELECT pr.tool_name, COUNT(*)
         FROM permission_requests pr LEFT JOIN sessions s ON s.session_id = pr.session_id
         WHERE {}
         GROUP BY pr.tool_name",
        where_.clause
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(where_.params), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (name, count) = row?;
        entry(&mut by_tool, name).permission_requests = count as u64;
    }

    let mut tools: Vec<ToolWaiting> = by_tool
        .into_values()
        .filter(|t| t.waiting > Duration::zero() || t.permission_requests > 0)
        .collect();
    tools.sort_by(|a, b| {
        b.waiting
            .cmp(&a.waiting)
            .then(b.permission_requests.cmp(&a.permission_requests))
            .then_with(|| a.tool_name.cmp(&b.tool_name))
    });
    Ok(tools)
}

fn load_turns(conn: &Connection, clause: &str, params: Vec<Value>) -> Result<Vec<TurnTime>> {
    let sql = format!(
        "SELECT t.session_id, t.prompt_id, t.prompt_text, {TURN_START}, {TURN_END}
         FROM turns t
         LEFT JOIN sessions s ON s.session_id = t.session_id
         LEFT JOIN api_messages m
                ON m.session_id = t.session_id AND m.prompt_id = t.prompt_id
         WHERE {TURN_START} IS NOT NULL AND {TURN_END} IS NOT NULL
           AND {TURN_END} >= {TURN_START} AND {clause}
         GROUP BY t.session_id, t.prompt_id
         ORDER BY {TURN_START}, t.session_id, t.prompt_id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut calls_stmt = conn.prepare(
        "SELECT tool_name, pre_at_us, post_at_us, duration_ms FROM tool_calls
         WHERE session_id = ?1 AND prompt_id = ?2 AND agent_id IS NULL
           AND post_at_us IS NOT NULL",
    )?;
    let mut turns = Vec::new();
    for row in rows {
        let (session_id, prompt_id, prompt_text, start_us, end_us) = row?;
        let calls = calls_stmt
            .query_map(params![session_id, prompt_id], |row| {
                Ok(CallTimes::new(
                    &row.get::<_, String>(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let (split, segments) = decompose(start_us, end_us, &calls);
        turns.push(TurnTime {
            session_id,
            prompt_id,
            prompt_text,
            start: clock::from_micros(start_us),
            end: clock::from_micros(end_us),
            split,
            segments,
        });
    }
    Ok(turns)
}

/// A main-thread tool call's intervals, in µs.
struct CallTimes {
    subagent: bool,
    /// `[post − duration, post]`; `[pre, post]` without a duration.
    exec: Option<(i64, i64)>,
    /// `[pre, exec start]`, when positive.
    wait: Option<(i64, i64)>,
}

impl CallTimes {
    fn new(tool_name: &str, pre: Option<i64>, post: i64, duration_ms: Option<i64>) -> Self {
        let exec_start = match duration_ms {
            Some(ms) => Some(post - ms.max(0) * 1000),
            None => pre,
        };
        let exec = exec_start.map(|start| (start, post));
        let wait = match (pre, exec_start) {
            (Some(pre), Some(start)) if start > pre => Some((pre, start)),
            _ => None,
        };
        Self {
            subagent: SUBAGENT_TOOLS.contains(&tool_name),
            exec,
            wait,
        }
    }
}

/// Partitions `[start, end]` by the calls' intervals (see the module docs).
fn decompose(start: i64, end: i64, calls: &[CallTimes]) -> (TimeSplit, Vec<Segment>) {
    let clip = |(a, b): (i64, i64)| {
        let (a, b) = (a.max(start), b.min(end));
        (a < b).then_some((a, b))
    };
    let mut intervals: Vec<(SegmentKind, (i64, i64))> = Vec::new();
    for call in calls {
        if let Some(exec) = call.exec.and_then(clip) {
            let kind = if call.subagent {
                SegmentKind::Subagent
            } else {
                SegmentKind::Tool
            };
            intervals.push((kind, exec));
        }
        if let Some(wait) = call.wait.and_then(clip) {
            intervals.push((SegmentKind::Waiting, wait));
        }
    }

    let mut bounds: Vec<i64> = vec![start, end];
    for (_, (a, b)) in &intervals {
        bounds.extend([*a, *b]);
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut split = TimeSplit::default();
    let mut segments: Vec<Segment> = Vec::new();
    for pair in bounds.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        // The highest-priority kind covering the elementary interval.
        let kind = intervals
            .iter()
            .filter(|(_, (x, y))| *x <= a && *y >= b)
            .map(|(kind, _)| *kind)
            .max()
            .unwrap_or(SegmentKind::Model);
        *split.component(kind) += Duration::microseconds(b - a);
        match segments.last_mut() {
            Some(last) if last.kind == kind => last.end = clock::from_micros(b),
            _ => segments.push(Segment {
                kind,
                start: clock::from_micros(a),
                end: clock::from_micros(b),
            }),
        }
    }
    (split, segments)
}
