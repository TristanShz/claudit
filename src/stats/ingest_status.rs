//! Health of the ingest pipeline, for the dashboard's warning banner.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::clock;
use crate::ingest::transcripts::{META_BACKFILLED_AT, META_SKIPPED_LINES};

/// Archive-wide ingest state (not subject to filters).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct IngestStatus {
    /// Transcript lines of unknown shape skipped so far, over all runs.
    pub skipped_transcript_lines: u64,
    /// When the first full pass over the transcripts on disk finished.
    pub transcripts_backfilled_at: Option<DateTime<Utc>>,
}

pub fn ingest_status(conn: &Connection) -> Result<IngestStatus> {
    let meta = |key: &str| -> Result<Option<i64>> {
        let value: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(value.and_then(|v| v.parse().ok()))
    };
    Ok(IngestStatus {
        skipped_transcript_lines: meta(META_SKIPPED_LINES)?.unwrap_or(0).max(0) as u64,
        transcripts_backfilled_at: meta(META_BACKFILLED_AT)?.map(clock::from_micros),
    })
}
