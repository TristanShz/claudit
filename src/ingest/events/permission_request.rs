//! `PermissionRequest`: Claude Code is about to ask the user to allow a
//! tool call. The wait itself is measured on the tool call (PreToolUse to
//! execution start); this records that a prompt happened, and for which tool.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::Value;

use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct PermissionRequest {
    tool_name: String,
    #[serde(default)]
    tool_input: Option<Value>,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let request: PermissionRequest = match event.parse() {
        Ok(request) => request,
        Err(malformed) => return Ok(malformed),
    };
    conn.execute(
        "INSERT INTO permission_requests
             (session_id, prompt_id, agent_id, tool_name, tool_input, cwd, at_us)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(session_id, at_us, tool_name) DO NOTHING",
        params![
            event.session_id,
            request.prompt_id,
            request.agent_id,
            request.tool_name,
            request.tool_input.map(|input| input.to_string()),
            request.cwd,
            event.received_at_us(),
        ],
    )?;
    Ok(Projection::Applied)
}
