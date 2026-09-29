//! `claudit ingest`: loads new input (spool files, transcripts) into
//! the archive. Idempotent: input already ingested is skipped via byte
//! offsets, and normalized rows are deduplicated on natural keys. Secrets
//! are redacted and outputs dropped before anything is stored
//! (`crate::redact`).

pub mod events;
pub mod offsets;
pub mod reingest;
pub mod spool;
pub mod transcripts;

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::db;
use crate::paths::Paths;
use crate::redact;

/// What one ingest run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// Hook events archived into `raw_events`.
    pub events: u64,
    /// Spool lines that could not be parsed and were skipped (and logged).
    pub skipped_lines: u64,
    /// Archived events whose payload did not have the expected shape for
    /// their event, so no normalized row was derived (and logged).
    pub unprojected_events: u64,
    /// Complete transcript lines read (only lines new since the last run).
    pub transcript_lines: u64,
    /// Transcript lines of unknown shape, skipped (and logged).
    pub skipped_transcript_lines: u64,
}

impl IngestReport {
    fn absorb(&mut self, other: IngestReport) {
        self.events += other.events;
        self.skipped_lines += other.skipped_lines;
        self.unprojected_events += other.unprojected_events;
        self.transcript_lines += other.transcript_lines;
        self.skipped_transcript_lines += other.skipped_transcript_lines;
    }
}

/// Runs one ingest pass over every input source.
pub fn run(paths: &Paths) -> Result<IngestReport> {
    let mut conn = db::open(paths)?;
    ingest_all(&mut conn, paths)
}

/// What `claudit reingest` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReingestReport {
    /// The replay of the archived hook events.
    pub replay: reingest::Replay,
    /// The ingest pass that followed (transcripts re-read from the start,
    /// plus any new spool lines).
    pub ingest: IngestReport,
}

/// Rebuilds every derived table from `raw_events` and the transcripts still
/// on disk (see [`reingest`]).
///
/// Must run under the same single-writer lock as [`run`] once it exists
/// (#4): wrap this whole call, exactly like `run`.
pub fn reingest(paths: &Paths) -> Result<ReingestReport> {
    let mut conn = db::open(paths)?;
    let replay = reingest::reset_and_replay(&mut conn, paths)?;
    let ingest = ingest_all(&mut conn, paths)?;
    Ok(ReingestReport { replay, ingest })
}

fn ingest_all(conn: &mut Connection, paths: &Paths) -> Result<IngestReport> {
    let mut report = IngestReport::default();
    report.absorb(spool::ingest(conn, paths)?);
    report.absorb(transcripts::ingest(conn, paths)?);
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![
            redact::META_PATTERNS_VERSION,
            redact::patterns_version().to_string()
        ],
    )?;
    Ok(report)
}
