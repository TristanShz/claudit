//! `SessionStart`: a session started (or resumed, cleared, compacted).

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;

use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct SessionStart {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

/// Records how the session started. A session can receive several
/// SessionStarts (e.g. `startup`, then `compact`); the first one ingested
/// is kept, which on replay is the earliest received.
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let start: SessionStart = match event.parse() {
        Ok(start) => start,
        Err(malformed) => return Ok(malformed),
    };
    touch_session(conn, &event.session_id, start.cwd.as_deref())?;
    conn.execute(
        "UPDATE sessions SET source = COALESCE(source, ?2) WHERE session_id = ?1",
        params![event.session_id, start.source],
    )?;
    Ok(Projection::Applied)
}

/// Makes sure the session has a row (hooks can arrive before its
/// transcript), with its working directory. Time span, branch and version
/// stay owned by the transcripts.
pub(super) fn touch_session(conn: &Connection, session_id: &str, cwd: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (session_id, cwd) VALUES (?1, ?2)
         ON CONFLICT(session_id) DO UPDATE SET cwd = COALESCE(cwd, excluded.cwd)",
        params![session_id, cwd],
    )?;
    Ok(())
}
