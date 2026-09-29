//! `UserPromptSubmit`: the user submitted a prompt, which starts a turn.
//!
//! Turn keying: the turn is `(session_id, prompt_id)`, the same key the
//! transcripts use (`promptId`), so hook and transcript rows for one turn
//! meet in a single `turns` row whichever is ingested first. Claude Code
//! sends `prompt_id` on every hook from v2.1.196 on; a payload without one
//! (older versions) is only archived, and that turn keeps its transcript
//! timings (`start_at_us` / `end_at_us`), which readers fall back to.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;

use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct UserPromptSubmit {
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    permission_mode: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

/// Sets the turn's submit time (earliest delivery wins) and fills its
/// prompt text and permission mode where the transcript has not.
pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let submit: UserPromptSubmit = match event.parse() {
        Ok(submit) => submit,
        Err(malformed) => return Ok(malformed),
    };
    let Some(prompt_id) = submit.prompt_id else {
        return Ok(Projection::Ignored);
    };
    super::session_start::touch_session(conn, &event.session_id, submit.cwd.as_deref())?;
    conn.execute(
        "INSERT INTO turns (session_id, prompt_id, prompt_text, permission_mode, submit_at_us)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(session_id, prompt_id) DO UPDATE SET
             prompt_text     = COALESCE(prompt_text, excluded.prompt_text),
             permission_mode = COALESCE(permission_mode, excluded.permission_mode),
             submit_at_us    = MIN(COALESCE(submit_at_us, excluded.submit_at_us),
                                   excluded.submit_at_us)",
        params![
            event.session_id,
            prompt_id,
            submit.prompt.filter(|p| !p.is_empty()),
            submit.permission_mode,
            event.received_at_us()
        ],
    )?;
    Ok(Projection::Applied)
}
