//! Session list with headline metrics.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params_from_iter};
use serde::Serialize;

use super::Filter;
use super::consumption::{SESSION_COLUMNS, TokenTotals};
use crate::clock;

/// One session's headline metrics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
}

/// Sessions started in the filter's range, most recent first.
pub fn session_list(conn: &Connection, filter: &Filter) -> Result<Vec<SessionSummary>> {
    let where_ = filter.sql(&SESSION_COLUMNS)?;
    let sql = format!(
        "SELECT s.session_id, s.cwd, s.git_branch, s.version, s.first_at_us, s.last_at_us,
                (SELECT COUNT(*) FROM turns t WHERE t.session_id = s.session_id),
                (SELECT t.prompt_text FROM turns t
                  WHERE t.session_id = s.session_id AND t.prompt_text IS NOT NULL
                  ORDER BY COALESCE(t.submit_at_us, t.start_at_us) LIMIT 1),
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
            tokens: TokenTotals::from_row(row, 8)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
