//! Where the time goes: each main-thread turn's wall time split into model,
//! tool, waiting and subagent time.
//!
//! Only turns the hooks timed count: a turn needs both its
//! `UserPromptSubmit` and its `Stop` receive times. Sessions imported from
//! transcripts (recorded before `claudit install`) have no time at all:
//! transcript spans include permission prompts, idle time and background
//! work, so they would inflate every component. Per turn:
//! - **wall** = Stop receive time − UserPromptSubmit receive time;
//! - **subagent** = the union of the execution intervals
//!   `[post − duration_ms, post]` of the main-thread calls blocked on a
//!   subagent: `Agent` / `Task` calls, and blocking `TaskOutput` calls on a
//!   subagent of the session (see `WAITS_ON_SUBAGENT`). A background run is
//!   not main-thread time: its Agent call returns at once and it runs
//!   alongside the main thread. The subagent's own tool calls, which carry
//!   an `agent_id`, are never main-thread time;
//! - **tool** = the union of the other main-thread calls' execution
//!   intervals, minus subagent time; parallel calls overlap instead of
//!   adding up;
//! - **waiting** = the union of the calls' waiting intervals
//!   `[pre, post − duration_ms]` (PreToolUse until execution starts: a
//!   permission prompt), minus tool and subagent time;
//! - **model** = everything else in the turn.
//!
//! Only hook-timed calls (`PreToolUse` / `PostToolUse`) give intervals; a
//! call known only from its transcript is left to model time. There is one
//! `tool_calls` row per call whichever sources saw it, so nothing is
//! counted twice.
//!
//! Every interval is clipped to the turn, and each instant of the turn is
//! assigned to exactly one component (subagent > tool > waiting > model),
//! so these four components always sum to the turn's wall time exactly. The
//! same partition is exposed as positioned [`Segment`]s per turn, for
//! timelines.
//!
//! A fifth component falls outside the turns: **background**, the union of
//! the session's subagent runs' hook-timed active spans (each
//! `SubagentStart` paired with the next `SubagentStop`, so a paused run is
//! not counted while paused; typed runs only: Claude Code's internal agents
//! are typeless) minus the union of its turns. A
//! background launch returns at once and the main thread stops, idle,
//! until a hand-back starts the next turn: without it, a session that
//! delegates its work would show only the few seconds of each hand-back.
//! Each background stretch lies between two turns and counts against the
//! turn before it (its filters, its day), as [`Segment`]s after its end.

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
    /// Subagents running between turns, the main thread idle (never
    /// within a turn, so its priority is moot).
    Background,
}

impl SegmentKind {
    /// Every kind, in display order (legends, charts, split bars).
    pub const ALL: [SegmentKind; 5] = [
        SegmentKind::Model,
        SegmentKind::Tool,
        SegmentKind::Waiting,
        SegmentKind::Subagent,
        SegmentKind::Background,
    ];

    /// The kind's identifier: `model`, `tool`, `waiting`, `subagent` or
    /// `background` (as serialized; also the dashboard's CSS hook).
    pub fn name(self) -> &'static str {
        match self {
            SegmentKind::Model => "model",
            SegmentKind::Tool => "tool",
            SegmentKind::Waiting => "waiting",
            SegmentKind::Subagent => "subagent",
            SegmentKind::Background => "background",
        }
    }

    /// The kind's display label.
    pub fn label(self) -> &'static str {
        match self {
            SegmentKind::Model => "Model",
            SegmentKind::Tool => "Tools",
            SegmentKind::Waiting => "Waiting on you",
            SegmentKind::Subagent => "Subagents",
            SegmentKind::Background => "Background subagents",
        }
    }
}

/// A stretch of a turn, positioned in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub kind: SegmentKind,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// A wall time split into its five components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSplit {
    pub model: Duration,
    pub tool: Duration,
    pub waiting: Duration,
    pub subagent: Duration,
    /// After the turn: subagents running, the main thread idle.
    pub background: Duration,
}

impl Default for TimeSplit {
    fn default() -> Self {
        Self {
            model: Duration::zero(),
            tool: Duration::zero(),
            waiting: Duration::zero(),
            subagent: Duration::zero(),
            background: Duration::zero(),
        }
    }
}

impl TimeSplit {
    /// The sum of the components.
    pub fn wall(&self) -> Duration {
        self.model + self.tool + self.waiting + self.subagent + self.background
    }

    /// The component of `kind`.
    pub fn component(&self, kind: SegmentKind) -> Duration {
        match kind {
            SegmentKind::Model => self.model,
            SegmentKind::Tool => self.tool,
            SegmentKind::Waiting => self.waiting,
            SegmentKind::Subagent => self.subagent,
            SegmentKind::Background => self.background,
        }
    }

    fn component_mut(&mut self, kind: SegmentKind) -> &mut Duration {
        match kind {
            SegmentKind::Model => &mut self.model,
            SegmentKind::Tool => &mut self.tool,
            SegmentKind::Waiting => &mut self.waiting,
            SegmentKind::Subagent => &mut self.subagent,
            SegmentKind::Background => &mut self.background,
        }
    }
}

impl std::ops::AddAssign for TimeSplit {
    fn add_assign(&mut self, other: Self) {
        self.model += other.model;
        self.tool += other.tool;
        self.waiting += other.waiting;
        self.subagent += other.subagent;
        self.background += other.background;
    }
}

/// One main-thread turn's time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnTime {
    pub session_id: String,
    pub prompt_id: String,
    pub prompt_text: Option<String>,
    /// `UserPromptSubmit` and `Stop`: background time comes after `end`.
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub split: TimeSplit,
    /// The turn from `start` to `end`, tiled without gaps or overlaps
    /// (adjacent segments have different kinds), then its background
    /// segments, with idle gaps between them.
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

/// Which sessions the time reports cover: time is measured only on the
/// sessions the hooks recorded, never on imported ones.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeCoverage {
    /// Filtered sessions with hook data (time is measured on their turns).
    pub recorded_sessions: u64,
    /// Filtered sessions known only from their transcripts (recorded before
    /// `claudit install`): counted in consumption, not in time.
    pub imported_sessions: u64,
    /// The first `UserPromptSubmit` the hooks recorded (archive-wide, not
    /// filtered): when time measurement started, roughly when claudit was
    /// installed.
    pub hooks_since: Option<DateTime<Utc>>,
}

/// SQL over `tool_calls tc`: whether the call's execution is the main
/// thread waiting on a subagent. An `Agent` / `Task` call (a foreground run
/// lasts as long as the call; a background launch returns at once), or a
/// blocking `TaskOutput` whose `task_id` is a subagent of the session (the
/// main thread waits for that run to finish). Anything else (a `sleep`, a
/// `Monitor`) cannot be told apart reliably and stays tool time.
const WAITS_ON_SUBAGENT: &str = "(tc.tool_name IN ('Agent', 'Task')
     OR (tc.tool_name = 'TaskOutput' AND json_valid(tc.tool_input)
         AND COALESCE(json_extract(tc.tool_input, '$.block'), 1)
         AND EXISTS (SELECT 1 FROM subagent_runs r
                     WHERE r.session_id = tc.session_id
                       AND r.agent_id = json_extract(tc.tool_input, '$.task_id'))))";

/// Turn start and end: the `UserPromptSubmit` and `Stop` receive times.
/// Transcript times are never used for time: a transcript's span includes
/// permission prompts, idle time and background work.
const TURN_START: &str = "t.submit_at_us";
const TURN_END: &str = "t.stop_at_us";

/// Filter columns of `turns t LEFT JOIN sessions s LEFT JOIN api_messages m`.
pub(super) const TURN_COLUMNS: FilterColumns = FilterColumns {
    time_us: TURN_START,
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some("m.model"),
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
        total += turn.split;
        *days.entry(turn.start.date_naive()).or_default() += turn.split;
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

/// Recorded vs imported sessions in the filter (sessions as in
/// [`super::sessions::session_list`]), and when hook timing started.
pub fn time_coverage(conn: &Connection, filter: &Filter) -> Result<TimeCoverage> {
    let where_ = filter.sql(&super::consumption::SESSION_COLUMNS)?;
    let (imported, recorded): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(imported), 0), COALESCE(SUM(NOT imported), 0)
             FROM (SELECT {} AS imported
                   FROM sessions s LEFT JOIN api_messages m ON m.session_id = s.session_id
                   WHERE s.first_at_us IS NOT NULL AND {}
                   GROUP BY s.session_id)",
            super::sessions::IMPORTED,
            where_.clause
        ),
        params_from_iter(where_.params),
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    // Indexed (`turns_by_submit`): no scan.
    let since: Option<i64> =
        conn.query_row("SELECT MIN(submit_at_us) FROM turns", [], |row| row.get(0))?;
    Ok(TimeCoverage {
        recorded_sessions: recorded.max(0) as u64,
        imported_sessions: imported.max(0) as u64,
        hooks_since: since.map(clock::from_micros),
    })
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
    // Hook-timed calls only: a transcript's tool_use → tool_result span
    // includes permission prompts.
    let mut calls_stmt = conn.prepare(&format!(
        "SELECT {WAITS_ON_SUBAGENT}, hook_pre_at_us, hook_post_at_us, hook_duration_ms
         FROM tool_calls tc
         WHERE session_id = ?1 AND prompt_id = ?2 AND agent_id IS NULL
           AND hook_post_at_us IS NOT NULL"
    ))?;
    let mut turns: Vec<TurnTime> = Vec::new();
    for row in rows {
        let (session_id, prompt_id, prompt_text, start_us, end_us) = row?;
        let calls = calls_stmt
            .query_map(params![session_id, prompt_id], |row| {
                Ok(CallTimes::new(
                    row.get::<_, bool>(0)?,
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
    add_background(conn, &mut turns)?;
    Ok(turns)
}

/// Adds each session's background time (see the module docs) to the turn
/// before each stretch, when that turn is among `turns`. The stretches are
/// computed against all the session's timed turns, filtered or not.
fn add_background(conn: &Connection, turns: &mut [TurnTime]) -> Result<()> {
    let mut by_session: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, turn) in turns.iter().enumerate() {
        by_session
            .entry(turn.session_id.clone())
            .or_default()
            .push(i);
    }
    let mut turns_stmt = conn.prepare(&format!(
        "SELECT t.prompt_id, {TURN_START}, {TURN_END} FROM turns t
         WHERE t.session_id = ?1 AND {TURN_START} IS NOT NULL AND {TURN_END} IS NOT NULL
           AND {TURN_END} >= {TURN_START}
         ORDER BY {TURN_START}, t.prompt_id"
    ))?;
    let mut agents_stmt = conn.prepare(
        "SELECT agent_id FROM subagent_runs
         WHERE session_id = ?1 AND agent_type IS NOT NULL AND agent_type <> ''",
    )?;
    let mut events_stmt = conn.prepare(
        "SELECT event, at_us FROM subagent_events WHERE agent_id = ?1 ORDER BY at_us, event",
    )?;
    for (session_id, indices) in by_session {
        let agents = agents_stmt
            .query_map([&session_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut runs: Vec<(i64, i64)> = Vec::new();
        for agent_id in &agents {
            for (start, stop) in super::subagents::active_spans(&mut events_stmt, agent_id)? {
                runs.push((clock::to_micros(start), clock::to_micros(stop)));
            }
        }
        if runs.is_empty() {
            continue;
        }
        let all_turns = turns_stmt
            .query_map([&session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<(String, i64, i64)>, _>>()?;
        let busy = union(all_turns.iter().map(|(_, a, b)| (*a, *b)).collect());
        for (a, b) in subtract(&union(runs), &busy) {
            // The turn before the stretch: the last one started by then.
            let Some((prompt_id, _, _)) = all_turns.iter().rev().find(|(_, s, _)| *s <= a) else {
                continue;
            };
            let Some(&i) = indices.iter().find(|&&i| &turns[i].prompt_id == prompt_id) else {
                continue;
            };
            let turn = &mut turns[i];
            turn.split.background += Duration::microseconds(b - a);
            turn.segments.push(Segment {
                kind: SegmentKind::Background,
                start: clock::from_micros(a),
                end: clock::from_micros(b),
            });
        }
    }
    Ok(())
}

/// The union of `intervals`, sorted and disjoint (touching ones merged).
fn union(mut intervals: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    intervals.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (a, b) in intervals {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

/// `from` minus `cut`, both sorted and disjoint; the result is too.
fn subtract(from: &[(i64, i64)], cut: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for &(mut a, b) in from {
        for &(x, y) in cut {
            if y <= a || x >= b {
                continue;
            }
            if x > a {
                out.push((a, x));
            }
            a = a.max(y);
            if a >= b {
                break;
            }
        }
        if a < b {
            out.push((a, b));
        }
    }
    out
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
    fn new(subagent: bool, pre: Option<i64>, post: i64, duration_ms: Option<i64>) -> Self {
        let exec_start = match duration_ms {
            Some(ms) => Some(clock::execution_start_us(post, ms)),
            None => pre,
        };
        let exec = exec_start.map(|start| (start, post));
        let wait = match (pre, exec_start) {
            (Some(pre), Some(start)) if start > pre => Some((pre, start)),
            _ => None,
        };
        Self {
            subagent,
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
        *split.component_mut(kind) += Duration::microseconds(b - a);
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
