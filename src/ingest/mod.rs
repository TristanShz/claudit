//! `claudit ingest`: loads new input (spool files, transcripts) into
//! the archive. Idempotent: input already ingested is skipped via byte
//! offsets, and normalized rows are deduplicated on natural keys. Secrets
//! are redacted and outputs dropped before anything is stored
//! (`crate::redact`).

mod bash_command;
pub mod events;
mod lock;
pub mod offsets;
mod purge;
pub mod reingest;
pub mod spool;
pub mod transcripts;

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::clock::Clock;
use crate::db;
use crate::paths::Paths;
use crate::redact;

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

/// Runs one ingest pass over every input source, without the lock: callers
/// other than [`catch_up`] must hold an [`IngestLock`] themselves.
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

/// How long `reingest` waits for a running ingest to release the lock.
const REINGEST_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Rebuilds every derived table from `raw_events` and the transcripts still
/// on disk (see [`reingest`]). Unlike [`catch_up`] it waits for a running
/// ingest to finish instead of leaving the work to it.
pub fn reingest(paths: &Paths) -> Result<ReingestReport> {
    let deadline = std::time::Instant::now() + REINGEST_LOCK_WAIT;
    let _lock = loop {
        if let Some(lock) = IngestLock::try_acquire(paths)? {
            break lock;
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "another ingest is still running; retry `claudit reingest` later"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
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
