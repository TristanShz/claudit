//! `subagent_runs` upserts, shared by every source that knows something
//! about a subagent run: SubagentStart / SubagentStop hooks, the parent's
//! Agent tool response, and the subagent's transcript (and its
//! `.meta.json`). Each source fills what it knows; merging is
//! order-independent (first non-empty value, earliest start, latest stop).

use anyhow::Result;
use rusqlite::{Connection, params};

/// What one source knows about a run. `None` means "unknown here".
#[derive(Debug, Default)]
pub(crate) struct RunUpdate<'a> {
    pub agent_id: &'a str,
    pub session_id: &'a str,
    pub prompt_id: Option<&'a str>,
    pub agent_type: Option<&'a str>,
    pub parent_tool_use_id: Option<&'a str>,
    pub model: Option<&'a str>,
    pub started_at_us: Option<i64>,
    pub stopped_at_us: Option<i64>,
    pub total_duration_ms: Option<i64>,
    pub total_tool_use_count: Option<i64>,
}

/// Records a `SubagentStart` (`start`) or `SubagentStop` (`stop`) receive
/// time in `subagent_events` (idempotent).
pub(crate) fn record_event(
    conn: &Connection,
    agent_id: &str,
    session_id: &str,
    prompt_id: Option<&str>,
    event: &str,
    at_us: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO subagent_events (agent_id, session_id, prompt_id, event, at_us)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![agent_id, session_id, prompt_id, event, at_us],
    )?;
    Ok(())
}

pub(crate) fn upsert(conn: &Connection, run: &RunUpdate) -> Result<()> {
    fn non_empty(s: Option<&str>) -> Option<&str> {
        s.filter(|s| !s.is_empty())
    }
    conn.execute(
        "INSERT INTO subagent_runs (
             agent_id, session_id, prompt_id, agent_type, parent_tool_use_id, model,
             started_at_us, stopped_at_us, total_duration_ms, total_tool_use_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(agent_id) DO UPDATE SET
             prompt_id            = COALESCE(prompt_id, excluded.prompt_id),
             agent_type           = COALESCE(agent_type, excluded.agent_type),
             parent_tool_use_id   = COALESCE(parent_tool_use_id, excluded.parent_tool_use_id),
             model                = COALESCE(model, excluded.model),
             started_at_us        = MIN(COALESCE(started_at_us, excluded.started_at_us),
                                        COALESCE(excluded.started_at_us, started_at_us)),
             stopped_at_us        = MAX(COALESCE(stopped_at_us, excluded.stopped_at_us),
                                        COALESCE(excluded.stopped_at_us, stopped_at_us)),
             total_duration_ms    = COALESCE(total_duration_ms, excluded.total_duration_ms),
             total_tool_use_count = COALESCE(total_tool_use_count, excluded.total_tool_use_count)",
        params![
            run.agent_id,
            run.session_id,
            non_empty(run.prompt_id),
            non_empty(run.agent_type),
            non_empty(run.parent_tool_use_id),
            non_empty(run.model),
            run.started_at_us,
            run.stopped_at_us,
            run.total_duration_ms,
            run.total_tool_use_count,
        ],
    )?;
    Ok(())
}
