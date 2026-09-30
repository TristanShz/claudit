//! Subagents: runs, and their aggregate per type.
//!
//! A run merges what hooks (SubagentStart/Stop, the parent's Agent tool
//! response) and the subagent's transcript know about it:
//! - **duration**: the run's **active spans**, each `SubagentStart` paired
//!   with the next `SubagentStop` (receive times), summed: a background run
//!   (the Agent call returns at once) and a run resumed later (a
//!   SendMessage) are measured as they really ran. Else Claude Code's
//!   `totalDurationMs` (the parent's Agent tool response); else none. Never
//!   the Agent call's own duration (a background launch returns in
//!   milliseconds) nor a transcript span (it includes the permission
//!   prompts inside the run);
//! - **tool calls**: Claude Code's `totalToolUseCount`, else the tool calls
//!   hooks recorded with the run's `agent_id`;
//! - **model**: the Agent response's `resolvedModel`, else the model of
//!   most of the run's API messages;
//! - **tokens**: the run's API messages (its transcript).
//!
//! Runs without a type (Claude Code's internal agents) are left out.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

use super::consumption::TokenTotals;
use super::{Filter, FilterColumns};
use crate::clock;

/// One subagent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRun {
    pub agent_id: String,
    pub session_id: String,
    /// The parent turn.
    pub prompt_id: Option<String>,
    pub agent_type: String,
    /// The Agent/Task call that ran it.
    pub parent_tool_use_id: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    /// Its active time (see the module docs).
    pub duration: Option<Duration>,
    /// Its hook-timed active spans (start, stop), oldest first; empty when
    /// the hooks saw no start/stop pair.
    pub active: Vec<(DateTime<Utc>, DateTime<Utc>)>,
    /// The `description` of the Agent call that ran it.
    pub description: Option<String>,
    pub tool_calls: u64,
    pub tokens: TokenTotals,
}

/// One subagent type's aggregate over the filtered runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentTypeStat {
    pub agent_type: String,
    pub runs: u64,
    pub total_duration: Duration,
    pub tool_calls: u64,
    /// The model most of its runs used.
    pub model: Option<String>,
    pub tokens: TokenTotals,
}

/// Filter columns of the `runs r LEFT JOIN sessions s` query below.
const RUN_COLUMNS: FilterColumns = FilterColumns {
    time_us: "r.started_at_us",
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some("r.model"),
};

/// A display name per run of `runs`: its Agent call's description, else
/// its type (for labelling a subagent's messages).
pub(super) fn display_names(runs: &[SubagentRun]) -> std::collections::HashMap<String, String> {
    runs.iter()
        .map(|r| {
            (
                r.agent_id.clone(),
                r.description
                    .clone()
                    .unwrap_or_else(|| r.agent_type.clone()),
            )
        })
        .collect()
}

/// Filtered runs, oldest first.
pub fn subagent_runs(conn: &Connection, filter: &Filter) -> Result<Vec<SubagentRun>> {
    let where_ = filter.sql(&RUN_COLUMNS)?;
    load_runs(conn, &where_.clause, where_.params)
}

/// Every run of one session, oldest first.
pub fn session_subagent_runs(conn: &Connection, session_id: &str) -> Result<Vec<SubagentRun>> {
    load_runs(
        conn,
        "r.session_id = ?",
        vec![Value::Text(session_id.to_owned())],
    )
}

/// Subagent types by runs, then total duration (descending), then name.
pub fn subagent_ranking(conn: &Connection, filter: &Filter) -> Result<Vec<SubagentTypeStat>> {
    let mut by_type: BTreeMap<String, (SubagentTypeStat, BTreeMap<String, u64>)> = BTreeMap::new();
    for run in subagent_runs(conn, filter)? {
        let (stat, models) = by_type.entry(run.agent_type.clone()).or_insert_with(|| {
            (
                SubagentTypeStat {
                    agent_type: run.agent_type.clone(),
                    runs: 0,
                    total_duration: Duration::zero(),
                    tool_calls: 0,
                    model: None,
                    tokens: TokenTotals::default(),
                },
                BTreeMap::new(),
            )
        });
        stat.runs += 1;
        stat.total_duration += run.duration.unwrap_or_else(Duration::zero);
        stat.tool_calls += run.tool_calls;
        stat.tokens.input += run.tokens.input;
        stat.tokens.output += run.tokens.output;
        stat.tokens.cache_write += run.tokens.cache_write;
        stat.tokens.cache_read += run.tokens.cache_read;
        if let Some(model) = run.model {
            *models.entry(model).or_default() += 1;
        }
    }
    let mut stats: Vec<SubagentTypeStat> = by_type
        .into_values()
        .map(|(mut stat, models)| {
            // Most runs first; ties go to the first name.
            stat.model = models
                .into_iter()
                .fold(None, |best: Option<(String, u64)>, (model, n)| match best {
                    Some((_, best_n)) if best_n >= n => best,
                    _ => Some((model, n)),
                })
                .map(|(model, _)| model);
            stat
        })
        .collect();
    stats.sort_by(|a, b| {
        b.runs
            .cmp(&a.runs)
            .then(b.total_duration.cmp(&a.total_duration))
            .then_with(|| a.agent_type.cmp(&b.agent_type))
    });
    Ok(stats)
}

fn load_runs(conn: &Connection, clause: &str, params: Vec<Value>) -> Result<Vec<SubagentRun>> {
    let sql = format!(
        "WITH runs AS (
             SELECT r.agent_id, r.session_id, r.prompt_id, r.agent_type, r.parent_tool_use_id,
                    COALESCE(r.model,
                             (SELECT m.model FROM api_messages m WHERE m.agent_id = r.agent_id
                              GROUP BY m.model ORDER BY COUNT(*) DESC, m.model LIMIT 1)) AS model,
                    r.started_at_us,
                    r.total_duration_ms AS duration_ms,
                    (SELECT json_extract(tc.tool_input, '$.description') FROM tool_calls tc
                     WHERE tc.tool_use_id = r.parent_tool_use_id
                       AND json_valid(tc.tool_input)) AS description,
                    COALESCE(r.total_tool_use_count,
                             (SELECT COUNT(*) FROM tool_calls tc
                              WHERE tc.agent_id = r.agent_id AND tc.post_at_us IS NOT NULL))
                        AS tool_calls
             FROM subagent_runs r
             WHERE r.agent_type IS NOT NULL AND r.agent_type <> ''
         )
         SELECT r.agent_id, r.session_id, r.prompt_id, r.agent_type, r.parent_tool_use_id,
                r.model, r.started_at_us, r.duration_ms, r.tool_calls, r.description, {}
         FROM runs r
         LEFT JOIN sessions s ON s.session_id = r.session_id
         LEFT JOIN api_messages m ON m.agent_id = r.agent_id AND m.session_id = r.session_id
         WHERE {clause}
         GROUP BY r.agent_id
         ORDER BY r.started_at_us, r.agent_id",
        TokenTotals::SUMS
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut events = conn.prepare(
        "SELECT event, at_us FROM subagent_events WHERE agent_id = ?1 ORDER BY at_us, event",
    )?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        Ok(SubagentRun {
            agent_id: row.get(0)?,
            session_id: row.get(1)?,
            prompt_id: row.get(2)?,
            agent_type: row.get(3)?,
            parent_tool_use_id: row.get(4)?,
            model: row.get(5)?,
            started_at: row.get::<_, Option<i64>>(6)?.map(clock::from_micros),
            duration: row
                .get::<_, Option<i64>>(7)?
                .map(|ms| Duration::milliseconds(ms.max(0))),
            tool_calls: row.get::<_, i64>(8)?.max(0) as u64,
            description: row.get::<_, Option<String>>(9).unwrap_or(None),
            tokens: TokenTotals::from_row(row, 10)?,
            active: Vec::new(),
        })
    })?;
    let mut runs: Vec<SubagentRun> = rows.collect::<Result<_, _>>()?;
    for run in &mut runs {
        let spans = active_spans(&mut events, &run.agent_id)?;
        if !spans.is_empty() {
            run.duration = Some(spans.iter().fold(Duration::zero(), |sum, (start, stop)| {
                sum + (*stop - *start)
            }));
        }
        run.active = spans;
    }
    Ok(runs)
}

/// The hook-timed active spans of a run: each `SubagentStart` paired with
/// the next `SubagentStop`. A stop without a start before it (the hooks
/// were installed while the run was going) and a start without a stop (a
/// run still going, or interrupted) give no span.
pub(super) fn active_spans(
    events: &mut rusqlite::Statement<'_>,
    agent_id: &str,
) -> Result<Vec<(DateTime<Utc>, DateTime<Utc>)>> {
    let mut spans = Vec::new();
    let mut open: Option<i64> = None;
    let mut rows = events.query([agent_id])?;
    while let Some(row) = rows.next()? {
        let (event, at_us): (String, i64) = (row.get(0)?, row.get(1)?);
        match (event.as_str(), open) {
            ("start", None) => open = Some(at_us),
            ("stop", Some(start)) => {
                spans.push((clock::from_micros(start), clock::from_micros(at_us)));
                open = None;
            }
            _ => {}
        }
    }
    Ok(spans)
}
