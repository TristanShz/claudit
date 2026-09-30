//! Tool rankings: by tool, by Bash leading command and by MCP server.
//!
//! Every ranking counts completed calls, successful or failed, and orders
//! rows by call count, then total duration (both descending), then name.
//!
//! Percentiles use the **nearest-rank** method over the calls that report a
//! duration: the p-th percentile of `n` sorted durations is the value at
//! 1-based rank `ceil(p / 100 × n)`. It is always an observed duration, and
//! the median of an even count is the lower of the two middle values.
//!
//! Filters: the date range applies to the call's completion time; project
//! matches the call's own `cwd` (else its session's); branch matches its
//! session's git branch (calls of a session without transcript have none);
//! model matches the model its turn — or, inside a subagent, its subagent
//! thread — first called.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use serde::Serialize;

use super::{Filter, FilterColumns};

/// Aggregate of a group of tool calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CallStats {
    /// Completed calls, successful or failed.
    pub calls: u64,
    /// Calls that ended in `PostToolUseFailure`.
    pub failures: u64,
    /// Sum of the calls' execution time (`duration_ms`).
    pub total_duration_ms: u64,
    /// Nearest-rank median execution time (`None` if no call reported one).
    pub median_duration_ms: Option<u64>,
    /// Nearest-rank 95th percentile execution time.
    pub p95_duration_ms: Option<u64>,
}

impl CallStats {
    /// Share of calls that failed, in `[0, 1]` (0 when there are no calls).
    pub fn failure_rate(&self) -> f64 {
        if self.calls == 0 {
            0.0
        } else {
            self.failures as f64 / self.calls as f64
        }
    }
}

/// One row of a ranking: a tool, a Bash command or an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RankedCalls {
    pub name: String,
    pub stats: CallStats,
}

/// When a call of `tool_calls tc` started executing, in SQL: the rule of
/// [`crate::clock::execution_start_us`] (NULL without a duration).
pub(super) const EXECUTION_START: &str = "(tc.post_at_us - MAX(tc.duration_ms, 0) * 1000)";

/// Filter columns of `tool_calls tc LEFT JOIN sessions s`.
pub(super) const TOOL_CALL_COLUMNS: FilterColumns = FilterColumns {
    time_us: "tc.post_at_us",
    cwd: Some("COALESCE(tc.cwd, s.cwd)"),
    branch: Some("s.git_branch"),
    model: Some(first_model_of_turn!("tc")),
};

/// Tools ranked by call count, with durations and failure rate.
pub fn tool_ranking(conn: &Connection, filter: &Filter) -> Result<Vec<RankedCalls>> {
    ranking(conn, filter, "tool_name")
}

/// Bash calls grouped by leading command (`git`, `cargo`, …). Calls whose
/// command could not be derived are left out.
pub fn bash_command_ranking(conn: &Connection, filter: &Filter) -> Result<Vec<RankedCalls>> {
    ranking(conn, filter, "bash_command")
}

/// MCP tool calls grouped by server (`mcp__<server>__<tool>`).
pub fn mcp_server_ranking(conn: &Connection, filter: &Filter) -> Result<Vec<RankedCalls>> {
    ranking(conn, filter, "mcp_server")
}

/// Ranks the completed calls in `filter` by `key` (a trusted column name).
fn ranking(conn: &Connection, filter: &Filter, key: &str) -> Result<Vec<RankedCalls>> {
    let where_ = filter.sql(&TOOL_CALL_COLUMNS)?;
    rank_where(conn, key, &where_.clause, where_.params)
}

/// Every tool of one session, its subagents' calls included.
pub fn session_tool_ranking(conn: &Connection, session_id: &str) -> Result<Vec<RankedCalls>> {
    rank_where(
        conn,
        "tool_name",
        "tc.session_id = ?",
        vec![Value::Text(session_id.to_owned())],
    )
}

/// Groups the completed calls matching `clause` by `key` (a trusted column
/// of `tool_calls tc`).
fn rank_where(
    conn: &Connection,
    key: &str,
    clause: &str,
    params: Vec<Value>,
) -> Result<Vec<RankedCalls>> {
    let sql = format!(
        "SELECT tc.{key}, tc.success, tc.duration_ms
         FROM tool_calls tc LEFT JOIN sessions s ON s.session_id = tc.session_id
         WHERE tc.post_at_us IS NOT NULL AND tc.{key} IS NOT NULL AND {clause}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;

    #[derive(Default)]
    struct Group {
        calls: u64,
        failures: u64,
        durations: Vec<u64>,
    }
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let group = groups.entry(row.get(0)?).or_default();
        group.calls += 1;
        if row.get::<_, Option<i64>>(1)? == Some(0) {
            group.failures += 1;
        }
        if let Some(ms) = row.get::<_, Option<i64>>(2)? {
            group.durations.push(ms.max(0) as u64);
        }
    }

    let mut ranking: Vec<RankedCalls> = groups
        .into_iter()
        .map(|(name, mut group)| {
            group.durations.sort_unstable();
            RankedCalls {
                name,
                stats: CallStats {
                    calls: group.calls,
                    failures: group.failures,
                    total_duration_ms: group.durations.iter().sum(),
                    median_duration_ms: nearest_rank(&group.durations, 50),
                    p95_duration_ms: nearest_rank(&group.durations, 95),
                },
            }
        })
        .collect();
    ranking.sort_by(|a, b| {
        (b.stats.calls, b.stats.total_duration_ms)
            .cmp(&(a.stats.calls, a.stats.total_duration_ms))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(ranking)
}

/// The `percentile`-th nearest-rank percentile of ascending `sorted`.
fn nearest_rank(sorted: &[u64], percentile: u64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len() as u64;
    let rank = (percentile * n).div_ceil(100).max(1);
    Some(sorted[(rank - 1) as usize])
}
