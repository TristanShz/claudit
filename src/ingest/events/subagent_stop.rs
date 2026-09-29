//! `SubagentStop`: a subagent finished responding. Claude Code also fires
//! it for its own internal agents (prompt suggestions, `/btw`), with an
//! empty `agent_type`; those runs stay typeless and reports skip them.

use anyhow::Result;
use rusqlite::Connection;
use serde::Deserialize;

use super::subagent_runs::{self, RunUpdate};
use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct SubagentStop {
    agent_id: String,
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
}

pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let stop: SubagentStop = match event.parse() {
        Ok(stop) => stop,
        Err(malformed) => return Ok(malformed),
    };
    subagent_runs::upsert(
        conn,
        &RunUpdate {
            agent_id: &stop.agent_id,
            session_id: &event.session_id,
            prompt_id: stop.prompt_id.as_deref(),
            agent_type: stop.agent_type.as_deref(),
            stopped_at_us: Some(event.received_at_us()),
            ..RunUpdate::default()
        },
    )?;
    Ok(Projection::Applied)
}
