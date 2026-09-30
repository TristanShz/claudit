//! `UserPromptExpansion`: the user typed a `/command` (a skill, a custom
//! command or an MCP prompt) that expanded into the prompt. Recorded as a
//! skill invocation with trigger `user`.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;

use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct UserPromptExpansion {
    command_name: String,
    #[serde(default)]
    command_args: Option<String>,
    #[serde(default)]
    command_source: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
}

/// One invocation per prompt (`expansion:<prompt_id>`); without a
/// `prompt_id` the receive time identifies it.
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let expansion: UserPromptExpansion = match event.parse() {
        Ok(expansion) => expansion,
        Err(malformed) => return Ok(malformed),
    };
    let skill = expansion.command_name.trim_start_matches('/');
    if skill.is_empty() {
        return Ok(Projection::Malformed("empty command_name".into()));
    }
    let invocation_id = match &expansion.prompt_id {
        Some(prompt_id) => format!("expansion:{prompt_id}"),
        None => format!("expansion-at:{}", event.received_at_us()),
    };
    conn.execute(
        "INSERT INTO skill_invocations
             (session_id, invocation_id, prompt_id, agent_id, skill, trigger, source, args, at_us)
         VALUES (?1, ?2, ?3, ?4, ?5, 'user', ?6, ?7, ?8)
         ON CONFLICT(session_id, invocation_id) DO UPDATE SET
             at_us = MIN(at_us, excluded.at_us)",
        params![
            event.session_id,
            invocation_id,
            expansion.prompt_id,
            expansion.agent_id,
            skill,
            expansion.command_source,
            expansion.command_args.filter(|a| !a.is_empty()),
            event.received_at_us(),
        ],
    )?;
    Ok(Projection::Applied)
}
