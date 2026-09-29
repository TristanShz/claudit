//! `claudit ingest`: loads new input (spool files; transcripts later) into
//! the archive. Idempotent: input already ingested is skipped via byte
//! offsets, and normalized rows are deduplicated on natural keys.

pub mod events;
pub mod offsets;
pub mod spool;

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
}

impl IngestReport {
    fn absorb(&mut self, other: IngestReport) {
        self.events += other.events;
        self.skipped_lines += other.skipped_lines;
        self.unprojected_events += other.unprojected_events;
    }
}

/// Runs one ingest pass over every input source.
pub fn run(paths: &Paths) -> Result<IngestReport> {
    let mut conn = db::open(paths)?;
    let mut report = IngestReport::default();
    report.absorb(spool::ingest(&mut conn, paths)?);
    Ok(report)
}
