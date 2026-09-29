//! `claudit ingest`: loads new input (spool files; transcripts later) into
//! the archive. Idempotent: input already ingested is skipped via byte
//! offsets, and normalized rows are deduplicated on natural keys.

pub mod events;
mod lock;
pub mod offsets;
mod purge;
pub mod spool;

use anyhow::Result;

use crate::clock::Clock;
use crate::db;
use crate::paths::Paths;

pub use lock::IngestLock;
pub use purge::SPOOL_IDLE_PURGE_AFTER;

/// Upper bound on passes per catch-up, so input that never stops growing
/// cannot keep one ingest process alive forever (the next `Stop` resumes).
const MAX_PASSES: usize = 100;

/// What a locked catch-up did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// This run held the lock and ingested everything pending (the report
    /// sums every pass).
    Ran(IngestReport),
    /// Another ingest holds the lock; it will pick up whatever is pending,
    /// so this run did nothing.
    AlreadyRunning,
}

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

/// Runs one ingest pass over every input source, without the lock: callers
/// other than [`catch_up`] must hold an [`IngestLock`] themselves.
pub fn run(paths: &Paths) -> Result<IngestReport> {
    let mut conn = db::open(paths)?;
    let mut report = IngestReport::default();
    report.absorb(spool::ingest(&mut conn, paths)?);
    Ok(report)
}

/// The ingest `claudit ingest` and `claudit serve` run: takes the
/// single-writer lock (or returns at once if another run holds it), then
/// runs passes until one finds no new input, so input that arrived while a
/// competing run was locked out is never left behind. Finally purges the
/// spool files that are no longer needed.
pub fn catch_up(paths: &Paths, clock: &dyn Clock) -> Result<IngestOutcome> {
    let Some(_lock) = IngestLock::try_acquire(paths)? else {
        return Ok(IngestOutcome::AlreadyRunning);
    };
    let mut total = IngestReport::default();
    for _ in 0..MAX_PASSES {
        let pass = run(paths)?;
        if pass == IngestReport::default() {
            break;
        }
        total.absorb(pass);
    }
    purge::purge_spool(&db::open(paths)?, paths, clock)?;
    Ok(IngestOutcome::Ran(total))
}
