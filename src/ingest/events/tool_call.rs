//! What `PostToolUse` and `PostToolUseFailure` share: the completed-call
//! payload and its upsert into `tool_calls`, including the derived Bash
//! leading command and MCP server.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::Value;

use super::{Projection, RawEvent};
use crate::ingest::bash_command;

/// The fields of a completed tool call (success or failure).
#[derive(Debug, Deserialize)]
struct CompletedCall {
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
    /// `PostToolUseFailure` only.
    #[serde(default)]
    error: Option<String>,
}

/// How the call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    Success,
    Failure,
}

/// Upserts the call on `tool_use_id`. On duplicate delivery the earliest
/// receive time wins, so replay order never changes the result.
pub(super) fn project(conn: &Connection, event: &RawEvent, outcome: Outcome) -> Result<Projection> {
    let call: CompletedCall = match event.parse() {
        Ok(call) => call,
        Err(malformed) => return Ok(malformed),
    };
    let bash_command = match call.tool_name.as_str() {
        "Bash" => call
            .tool_input
            .as_ref()
            .and_then(|input| input.get("command"))
            .and_then(Value::as_str)
            .and_then(bash_command::leading_command),
        _ => None,
    };
    let mcp_server = mcp_server(&call.tool_name);
    let (success, error) = match outcome {
        Outcome::Success => (1, None),
        Outcome::Failure => (0, call.error),
    };
    conn.execute(
        "INSERT INTO tool_calls (
             tool_use_id, session_id, prompt_id, agent_id, tool_name, mcp_server,
             bash_command, tool_input, cwd, post_at_us, duration_ms, success, error
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(tool_use_id) DO UPDATE SET
             session_id   = excluded.session_id,
             prompt_id    = COALESCE(excluded.prompt_id, prompt_id),
             agent_id     = COALESCE(excluded.agent_id, agent_id),
             tool_name    = excluded.tool_name,
             mcp_server   = excluded.mcp_server,
             bash_command = excluded.bash_command,
             tool_input   = COALESCE(excluded.tool_input, tool_input),
             cwd          = COALESCE(excluded.cwd, cwd),
             post_at_us   = MIN(COALESCE(post_at_us, excluded.post_at_us), excluded.post_at_us),
             duration_ms  = COALESCE(excluded.duration_ms, duration_ms),
             success      = excluded.success,
             error        = excluded.error",
        params![
            call.tool_use_id,
            event.session_id,
            call.prompt_id,
            call.agent_id,
            call.tool_name,
            mcp_server,
            bash_command,
            call.tool_input.map(|input| input.to_string()),
            call.cwd,
            event.received_at_us(),
            call.duration_ms,
            success,
            error,
        ],
    )?;
    Ok(Projection::Applied)
}

/// The server of an MCP tool, from its name `mcp__<server>__<tool>`. Server
/// names may contain single underscores (`mcp__plugin_x_y__tool`), never a
/// double one, so the server is everything up to the next `__`.
fn mcp_server(tool_name: &str) -> Option<&str> {
    let rest = tool_name.strip_prefix("mcp__")?;
    let (server, _tool) = rest.split_once("__")?;
    (!server.is_empty()).then_some(server)
}
