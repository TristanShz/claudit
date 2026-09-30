//! `PreToolUse`: Claude is about to call a tool (before any permission
//! prompt), paired with its `PostToolUse` on `tool_use_id`.

use anyhow::Result;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;

use super::tool_call::{self, AnnouncedCall};
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

/// Records the call's `PreToolUse` time (earliest delivery wins) without
/// touching what `PostToolUse` or the transcript own, so any may be ingested
/// first. A call with only a PreToolUse (still running, or interrupted) has
/// no `post_at_us` and is left out of every tool report.
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let call: PreToolUse = match event.parse() {
        Ok(call) => call,
        Err(malformed) => return Ok(malformed),
    };
    tool_call::record_hook_pre(
        conn,
        &AnnouncedCall {
            tool_use_id: &call.tool_use_id,
            session_id: &event.session_id,
            prompt_id: call.prompt_id.as_deref(),
            agent_id: call.agent_id.as_deref(),
            tool_name: &call.tool_name,
            tool_input: call.tool_input.as_ref(),
            cwd: call.cwd.as_deref(),
            at_us: event.received_at_us(),
        },
    )?;
    Ok(Projection::Applied)
}
