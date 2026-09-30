//! `Notification`: Claude Code notified the user (permission prompt, idle
//! prompt, …).

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;

use super::{Projection, RawEvent};

#[derive(Debug, Deserialize)]
struct Notification {
    #[serde(default)]
    notification_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    let notification: Notification = match event.parse() {
        Ok(notification) => notification,
        Err(malformed) => return Ok(malformed),
    };
    conn.execute(
        "INSERT INTO notifications
             (session_id, prompt_id, notification_type, message, cwd, at_us)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(session_id, at_us, notification_type) DO NOTHING",
        params![
            event.session_id,
            notification.prompt_id,
            notification
                .notification_type
                .as_deref()
                .unwrap_or("unknown"),
            notification.message,
            notification.cwd,
            event.received_at_us(),
        ],
    )?;
    Ok(Projection::Applied)
}
