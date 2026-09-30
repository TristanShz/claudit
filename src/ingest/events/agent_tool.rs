//! `PostToolUse` of the `Agent` (formerly `Task`) tool: a subagent run
//! finished. Its `tool_response` carries the run's totals (`agentId`,
//! `agentType`, `totalDurationMs`, `totalToolUseCount`, `resolvedModel`),
//! which fill `subagent_runs` and link the run to its parent tool call.
//!
//! Reads the payload as archived: the redaction of tool outputs (#7) must
//! keep these `tool_response` fields for reingest to rebuild the same rows.

use anyhow::Result;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;

use super::RawEvent;
use super::subagent_runs::{self, RunUpdate};
use crate::clock;

#[derive(Debug, Deserialize)]
struct AgentCall {
    tool_use_id: String,
    tool_name: String,
    #[serde(default)]
    tool_input: Value,
    #[serde(default)]
    tool_response: Value,
    #[serde(default)]
    prompt_id: Option<String>,
}

/// No-op unless the event is an Agent/Task call whose response names the
/// subagent it ran.
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<()> {
    let Ok(call) = event.parse::<AgentCall>() else {
        return Ok(());
    };
    if !matches!(call.tool_name.as_str(), "Agent" | "Task") {
        return Ok(());
    }
    let response = &call.tool_response;
    let str_field = |key: &str| response.get(key).and_then(Value::as_str);
    let int_field = |key: &str| response.get(key).and_then(Value::as_i64);
    let Some(agent_id) = str_field("agentId") else {
        return Ok(());
    };
    let post_us = event.received_at_us();
    let duration_ms = int_field("totalDurationMs");
    subagent_runs::upsert(
        conn,
        &RunUpdate {
            agent_id,
            session_id: &event.session_id,
            prompt_id: call.prompt_id.as_deref(),
            agent_type: str_field("agentType")
                .or_else(|| call.tool_input.get("subagent_type").and_then(Value::as_str)),
            parent_tool_use_id: Some(&call.tool_use_id),
            model: str_field("resolvedModel"),
            started_at_us: duration_ms.map(|ms| clock::execution_start_us(post_us, ms)),
            stopped_at_us: Some(post_us),
            total_duration_ms: duration_ms,
            total_tool_use_count: int_field("totalToolUseCount"),
        },
    )
}
