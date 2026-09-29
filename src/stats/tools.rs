//! Tool ranking.

use anyhow::Result;
use rusqlite::{Connection, params_from_iter};
use serde::Serialize;

use super::{Filter, FilterColumns};

/// One tool's aggregate over the filtered calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolStat {
    pub tool_name: String,
    /// Completed calls.
    pub calls: u64,
    /// Sum of the calls' execution time (`duration_ms`).
    pub total_duration_ms: u64,
}

const COLUMNS: FilterColumns = FilterColumns {
    time_us: "post_at_us",
    cwd: Some("cwd"),
    branch: None,
    model: None,
};

/// Tools by call count, then total duration (descending), then name.
pub fn top_tools(conn: &Connection, filter: &Filter) -> Result<Vec<ToolStat>> {
    let where_ = filter.sql(&COLUMNS)?;
    let sql = format!(
        "SELECT tool_name, COUNT(*), COALESCE(SUM(duration_ms), 0) AS total
         FROM tool_calls
         WHERE post_at_us IS NOT NULL AND {}
         GROUP BY tool_name
         ORDER BY COUNT(*) DESC, total DESC, tool_name",
        where_.clause
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(where_.params), |row| {
        Ok(ToolStat {
            tool_name: row.get(0)?,
            calls: row.get::<_, i64>(1)? as u64,
            total_duration_ms: row.get::<_, i64>(2)? as u64,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
