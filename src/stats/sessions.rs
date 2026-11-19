//! Session list with headline metrics.
//!
//! A session is **imported** when the hooks recorded nothing of it: it is
//! known only from its transcripts, because it ran before `claudit install`.
//! Imported sessions have tokens, cost, turns and tool calls, but no time
//! (see [`super::time`]).

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params_from_iter};
use std::collections::HashMap;

use super::Filter;
use super::consumption::{SESSION_COLUMNS, TokenTotals};
use super::time::{self, TimeSplit, TurnTime};
use crate::clock;

/// One session's headline metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    /// Claude Code version.
    pub version: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub turns: u64,
    /// Its first typed prompt (a slash command included), else its first
    /// prompt, labelled (see [`super::prompt`]).
    pub first_prompt: Option<String>,
    /// Tokens of the session, subagents included (only the filtered model's
    /// under a model filter).
    pub tokens: TokenTotals,
    /// Completed tool calls, subagents' included.
    pub tool_calls: u64,
    /// Where the time of its turns went (only the turns the filter keeps:
    /// see [`super::time::turn_times`]). Zero for an imported session.
    pub time: TimeSplit,
    /// Known only from its transcripts: no hook recorded anything of it.
    pub imported: bool,
}

/// SQL: whether the session of alias `s` is imported (no hook event
/// archived for it).
pub(super) const IMPORTED: &str =
    "NOT EXISTS (SELECT 1 FROM raw_events r WHERE r.session_id = s.session_id)";

/// Sessions started in the filter's range, most recent first. Project and
/// branch match the session; a model filter keeps the sessions that used
/// the model (and only its tokens).
pub fn session_list(conn: &Connection, filter: &Filter) -> Result<Vec<SessionSummary>> {
    sessions_with_times(conn, filter, &time::turn_times(conn, filter)?, None)
}

/// [`session_list`] for a caller that already has the filter's
/// [`time::turn_times`] (`turns`), cut to the `limit` most recent sessions
/// when one is given.
pub fn sessions_with_times(
    conn: &Connection,
    filter: &Filter,
    turns: &[TurnTime],
    limit: Option<usize>,
) -> Result<Vec<SessionSummary>> {
    let where_ = filter.sql(&SESSION_COLUMNS)?;
    // The sessions are picked (and cut to `limit`) first, materialized, so
    // the per-session subqueries run only for the ones returned.
    let sql = format!(
        "WITH page (session_id, cwd, git_branch, version, first_at_us, last_at_us,
                    input_tokens, output_tokens, cache_write_tokens, cache_read_tokens)
         AS MATERIALIZED (
             SELECT s.session_id, s.cwd, s.git_branch, s.version, s.first_at_us, s.last_at_us,
                    {}
             FROM sessions s LEFT JOIN api_messages m ON m.session_id = s.session_id
             WHERE s.first_at_us IS NOT NULL AND {}
             GROUP BY s.session_id
             ORDER BY s.first_at_us DESC, s.session_id
             LIMIT {}
         )
         SELECT s.session_id, s.cwd, s.git_branch, s.version, s.first_at_us, s.last_at_us,
                (SELECT COUNT(*) FROM turns t WHERE t.session_id = s.session_id),
                (SELECT t.prompt_text FROM turns t
                  WHERE t.session_id = s.session_id AND t.prompt_text IS NOT NULL
                  ORDER BY {INJECTED}, COALESCE(t.submit_at_us, t.start_at_us) LIMIT 1),
                (SELECT COUNT(*) FROM tool_calls tc
                  WHERE tc.session_id = s.session_id AND tc.post_at_us IS NOT NULL),
                {IMPORTED},
                s.input_tokens, s.output_tokens, s.cache_write_tokens, s.cache_read_tokens
         FROM page s
         ORDER BY s.first_at_us DESC, s.session_id",
        TokenTotals::SUMS,
        where_.clause,
        // SQLite reads a negative LIMIT as no limit.
        limit.map_or(-1, |n| n.min(i64::MAX as usize) as i64),
        INJECTED = super::prompt::INJECTED_SQL,
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(where_.params), |row| {
        Ok(SessionSummary {
            session_id: row.get(0)?,
            cwd: row.get(1)?,
            git_branch: row.get(2)?,
            version: row.get(3)?,
            started_at: clock::from_micros(row.get(4)?),
            last_activity_at: clock::from_micros(row.get(5)?),
            turns: row.get::<_, i64>(6)? as u64,
            first_prompt: row
                .get::<_, Option<String>>(7)?
                .map(|text| super::prompt::label(&text, &HashMap::new()).text),
            tool_calls: row.get::<_, i64>(8)?.max(0) as u64,
            imported: row.get(9)?,
            tokens: TokenTotals::from_row(row, 10)?,
            time: TimeSplit::default(),
        })
    })?;
    let mut sessions: Vec<SessionSummary> = rows.collect::<Result<_, _>>()?;
    let mut times: HashMap<String, TimeSplit> = HashMap::new();
    for turn in turns {
        *times.entry(turn.session_id.clone()).or_default() += turn.split;
    }
    for session in &mut sessions {
        session.time = times.remove(&session.session_id).unwrap_or_default();
    }
    Ok(sessions)
}
