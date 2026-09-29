//! Hook events: archiving them in `raw_events` and projecting them into the
//! normalized tables.
//!
//! Projection is a function of one archived event, so `claudit reingest` can
//! rebuild every normalized table by replaying `raw_events` through
//! [`project`]. Each hook event type has its own handler module; to support a
//! new event, add a module and one arm to [`project`].

mod post_tool_use;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::clock;
use crate::spool::SpoolRecord;

/// A hook event as archived: payload plus the fields every event carries.
#[derive(Debug, Clone, PartialEq)]
pub struct RawEvent {
    pub session_id: String,
    pub hook_event_name: String,
    pub received_at: DateTime<Utc>,
    pub payload: Value,
}

impl RawEvent {
    pub fn from_spool(record: SpoolRecord) -> Result<Self> {
        Ok(Self {
            session_id: record.session_id()?.to_owned(),
            hook_event_name: record.hook_event_name()?.to_owned(),
            received_at: record.received_at,
            payload: record.payload,
        })
    }

    /// The receive time in storage format (µs since the epoch).
    pub fn received_at_us(&self) -> i64 {
        clock::to_micros(self.received_at)
    }

    /// Deserializes the payload into an event-specific shape.
    pub fn parse<T: DeserializeOwned>(&self) -> Result<T, Projection> {
        T::deserialize(&self.payload).map_err(|err| Projection::Malformed(err.to_string()))
    }
}

/// The result of projecting one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Projection {
    /// Normalized rows were written.
    Applied,
    /// The event type has no projection (yet); it is only archived.
    Ignored,
    /// The payload lacked what its event type requires.
    Malformed(String),
}

/// Appends the event to `raw_events`.
pub fn archive(conn: &Connection, event: &RawEvent) -> Result<()> {
    conn.execute(
        "INSERT INTO raw_events (session_id, hook_event_name, received_at_us, payload)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            event.session_id,
            event.hook_event_name,
            event.received_at_us(),
            event.payload.to_string()
        ],
    )?;
    Ok(())
}

/// Derives normalized rows from one event. Must be idempotent: replaying the
/// same event (duplicate delivery, reingest) never double-counts.
pub fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    match event.hook_event_name.as_str() {
        "PostToolUse" => post_tool_use::project(conn, event),
        _ => Ok(Projection::Ignored),
    }
}
