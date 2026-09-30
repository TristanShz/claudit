//! `SubagentStart`: a subagent started.

use anyhow::Result;
use rusqlite::Connection;
use serde::Deserialize;

use super::subagent_runs::{self, RunUpdate};
use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct SubagentStart {
    agent_id: String,
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
}

pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let start: SubagentStart = match event.parse() {
        Ok(start) => start,
        Err(malformed) => return Ok(malformed),
    };
    subagent_runs::upsert(
        conn,
        &RunUpdate {
            agent_id: &start.agent_id,
            session_id: &event.session_id,
            prompt_id: start.prompt_id.as_deref(),
            agent_type: start.agent_type.as_deref(),
            started_at_us: Some(event.received_at_us()),
            ..RunUpdate::default()
        },
    )?;
    subagent_runs::record_event(
        conn,
        &start.agent_id,
        &event.session_id,
        start.prompt_id.as_deref(),
        "start",
        event.received_at_us(),
    )?;
    Ok(Projection::Applied)
}
