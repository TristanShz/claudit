//! `claudit ingest`: loads new input (spool files, transcripts) into
//! the archive. Idempotent: input already ingested is skipped via byte
//! offsets, and normalized rows are deduplicated on natural keys.

pub mod events;
pub mod offsets;
pub mod spool;
pub mod transcripts;

use anyhow::Result;

use crate::db;
use crate::paths::Paths;

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
    let mut report = IngestReport::default();
    report.absorb(spool::ingest(&mut conn, paths)?);
    report.absorb(transcripts::ingest(&mut conn, paths)?);
    Ok(report)
}
