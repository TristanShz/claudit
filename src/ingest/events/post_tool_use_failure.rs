//! `PostToolUseFailure`: a tool call that started executing and failed. It
//! becomes a failed `tool_calls` row carrying the error text.

use anyhow::Result;
use rusqlite::Connection;

use super::tool_call::{self, Outcome};
use super::{Projection, RawEvent};

pub(super) fn project(conn: &Connection, event: &RawEvent) -> Result<Projection> {
    tool_call::project(conn, event, Outcome::Failure)
}
