//! `PreToolUse`: Claude is about to call a tool (before any permission
//! prompt), paired with its `PostToolUse` on `tool_use_id`.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::Value;

use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct PreToolUse {
    tool_use_id: String,
    tool_name: String,
    #[serde(default)]
    tool_input: Option<Value>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
}

/// Upserts the call's `pre_at_us` (earliest delivery wins) without
/// touching what `PostToolUse` owns, so either may be ingested first. A
/// call with only a PreToolUse (still running, or interrupted) has no
/// `post_at_us` and is left out of every tool report.
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let call: PreToolUse = match event.parse() {
        Ok(call) => call,
        Err(malformed) => return Ok(malformed),
    };
    conn.execute(
        "INSERT INTO tool_calls (
             tool_use_id, session_id, prompt_id, agent_id, tool_name, tool_input, cwd, pre_at_us
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(tool_use_id) DO UPDATE SET
             prompt_id  = COALESCE(prompt_id, excluded.prompt_id),
             agent_id   = COALESCE(agent_id, excluded.agent_id),
             tool_input = COALESCE(tool_input, excluded.tool_input),
             cwd        = COALESCE(cwd, excluded.cwd),
             pre_at_us  = MIN(COALESCE(pre_at_us, excluded.pre_at_us), excluded.pre_at_us)",
        params![
            call.tool_use_id,
            event.session_id,
            call.prompt_id,
            call.agent_id,
            call.tool_name,
            call.tool_input.map(|input| input.to_string()),
            call.cwd,
            event.received_at_us(),
        ],
    )?;
    Ok(Projection::Applied)
}
