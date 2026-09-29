//! `PostToolUse` of the `Skill` tool: Claude invoked a skill. Recorded as a
//! skill invocation with trigger `model` (the call itself is also a tool
//! call, projected by `post_tool_use`).

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::Value;

use super::RawEvent;

#[derive(Debug, Deserialize)]
struct SkillCall {
    tool_use_id: String,
    tool_name: String,
    #[serde(default)]
    tool_input: Value,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
}

/// No-op unless the event is a Skill tool call naming its skill
/// (`tool_input.skill`, or `command` in older versions).
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<()> {
    let Ok(call) = event.parse::<SkillCall>() else {
        return Ok(());
    };
    if call.tool_name != "Skill" {
        return Ok(());
    }
    let input = &call.tool_input;
    let Some(skill) = ["skill", "command"]
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .map(|s| s.trim_start_matches('/'))
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };
    conn.execute(
        "INSERT INTO skill_invocations
             (session_id, invocation_id, prompt_id, agent_id, skill, trigger, args, at_us)
         VALUES (?1, ?2, ?3, ?4, ?5, 'model', ?6, ?7)
         ON CONFLICT(session_id, invocation_id) DO UPDATE SET
             at_us = MIN(at_us, excluded.at_us)",
        params![
            event.session_id,
            call.tool_use_id,
            call.prompt_id,
            call.agent_id,
            skill,
            input.get("args").and_then(Value::as_str),
            event.received_at_us(),
        ],
    )?;
    Ok(())
}
