//! Session list with headline metrics.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params_from_iter};
use std::collections::HashMap;

use super::Filter;
use super::consumption::{SESSION_COLUMNS, TokenTotals};
use super::time::{self, TimeSplit};
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
    pub first_prompt: Option<String>,
    /// Tokens of the session, subagents included (only the filtered model's
    /// under a model filter).
    pub tokens: TokenTotals,
    /// Completed tool calls, subagents' included.
    pub tool_calls: u64,
    /// Where the time of its turns went (only the turns the filter keeps:
    /// see [`super::time::turn_times`]).
    pub time: TimeSplit,
}

/// Sessions started in the filter's range, most recent first. Project and
/// branch match the session; a model filter keeps the sessions that used
/// the model (and only its tokens).
pub fn session_list(conn: &Connection, filter: &Filter) -> Result<Vec<SessionSummary>> {
    let where_ = filter.sql(&SESSION_COLUMNS)?;
    let sql = format!(
        "SELECT s.session_id, s.cwd, s.git_branch, s.version, s.first_at_us, s.last_at_us,
                (SELECT COUNT(*) FROM turns t WHERE t.session_id = s.session_id),
                (SELECT t.prompt_text FROM turns t
                  WHERE t.session_id = s.session_id AND t.prompt_text IS NOT NULL
                  ORDER BY COALESCE(t.submit_at_us, t.start_at_us) LIMIT 1),
                (SELECT COUNT(*) FROM tool_calls tc
                  WHERE tc.session_id = s.session_id AND tc.post_at_us IS NOT NULL),
                {}
         FROM sessions s LEFT JOIN api_messages m ON m.session_id = s.session_id
         WHERE s.first_at_us IS NOT NULL AND {}
         GROUP BY s.session_id
         ORDER BY s.first_at_us DESC, s.session_id",
        TokenTotals::SUMS,
        where_.clause
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
            first_prompt: row.get(7)?,
            tool_calls: row.get::<_, i64>(8)?.max(0) as u64,
            tokens: TokenTotals::from_row(row, 9)?,
            time: TimeSplit::default(),
        })
    })?;
    let mut sessions: Vec<SessionSummary> = rows.collect::<Result<_, _>>()?;
    let mut times: HashMap<String, TimeSplit> = HashMap::new();
    for turn in time::turn_times(conn, filter)? {
        *times.entry(turn.session_id).or_default() += turn.split;
    }
    for session in &mut sessions {
        session.time = times.remove(&session.session_id).unwrap_or_default();
    }
    Ok(sessions)
}
