//! `Stop`: Claude finished responding, which ends the turn.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;

use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct Stop {
    #[serde(default)]
    prompt_id: Option<String>,
}

/// Sets the turn's stop time. A turn can stop more than once (a Stop hook
/// that blocks makes Claude continue), so the latest Stop wins. Keyed like
/// `UserPromptSubmit` (see there); without `prompt_id` it is only archived.
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let stop: Stop = match event.parse() {
        Ok(stop) => stop,
        Err(malformed) => return Ok(malformed),
    };
    let Some(prompt_id) = stop.prompt_id else {
        return Ok(Projection::Ignored);
    };
    conn.execute(
        "INSERT INTO turns (session_id, prompt_id, stop_at_us) VALUES (?1, ?2, ?3)
         ON CONFLICT(session_id, prompt_id) DO UPDATE SET
             stop_at_us = MAX(COALESCE(stop_at_us, excluded.stop_at_us), excluded.stop_at_us)",
        params![event.session_id, prompt_id, event.received_at_us()],
    )?;
    Ok(Projection::Applied)
}
