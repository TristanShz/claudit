//! `PostToolUse`: a tool call that completed successfully.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::Value;

use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct PostToolUse {
    tool_use_id: String,
    tool_name: String,
    #[serde(default)]
    tool_input: Option<Value>,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
}

/// Upserts the call on `tool_use_id`. On duplicate delivery the earliest
/// receive time wins, so replay order never changes the result.
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let call: PostToolUse = match event.parse() {
        Ok(call) => call,
        Err(malformed) => return Ok(malformed),
    };
    conn.execute(
        "INSERT INTO tool_calls (
             tool_use_id, session_id, prompt_id, agent_id, tool_name,
             tool_input, cwd, post_at_us, duration_ms, success
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1)
         ON CONFLICT(tool_use_id) DO UPDATE SET
             session_id  = excluded.session_id,
             prompt_id   = COALESCE(excluded.prompt_id, prompt_id),
             agent_id    = COALESCE(excluded.agent_id, agent_id),
             tool_name   = excluded.tool_name,
             tool_input  = COALESCE(excluded.tool_input, tool_input),
             cwd         = COALESCE(excluded.cwd, cwd),
             post_at_us  = MIN(COALESCE(post_at_us, excluded.post_at_us), excluded.post_at_us),
             duration_ms = COALESCE(excluded.duration_ms, duration_ms),
             success     = 1",
        params![
            call.tool_use_id,
            event.session_id,
            call.prompt_id,
            call.agent_id,
            call.tool_name,
            call.tool_input.map(|input| input.to_string()),
            call.cwd,
            event.received_at_us(),
            call.duration_ms,
        ],
    )?;
    Ok(Projection::Applied)
}
