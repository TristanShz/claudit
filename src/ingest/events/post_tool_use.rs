//! `PostToolUse`: a tool call that completed successfully.

use anyhow::Result;
use rusqlite::Connection;

use super::tool_call::{self, Outcome};
use super::{Projection, RawEvent};

pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    tool_call::project(conn, event, Outcome::Success)
}
