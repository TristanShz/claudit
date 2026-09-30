//! `SessionEnd`: the session ended.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;

use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct SessionEnd {
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

/// Records when and why the session ended (the latest end wins: a resumed
/// session ends again).
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let end: SessionEnd = match event.parse() {
        Ok(end) => end,
        Err(malformed) => return Ok(malformed),
    };
    super::session_start::touch_session(conn, &event.session_id, end.cwd.as_deref())?;
    conn.execute(
        "UPDATE sessions
         SET end_reason  = CASE WHEN ended_at_us IS NULL OR ?2 >= ended_at_us
                                THEN ?3 ELSE end_reason END,
             ended_at_us = MAX(COALESCE(ended_at_us, ?2), ?2)
         WHERE session_id = ?1",
        params![event.session_id, event.received_at_us(), end.reason],
    )?;
    Ok(Projection::Applied)
}
