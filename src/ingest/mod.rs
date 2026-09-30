//! `claudit ingest`: loads new input (spool files, transcripts) into
//! the archive. Idempotent: input already ingested is skipped via byte
//! offsets, and normalized rows are deduplicated on natural keys. Secrets
//! are redacted and outputs dropped before anything is stored
//! (`crate::redact`).

pub mod events;
mod lock;
pub mod offsets;
mod purge;
pub mod reingest;
pub mod spool;
pub mod transcripts;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::clock::Clock;
use crate::db;
use crate::logfile;
use crate::paths::Paths;
use crate::redact;

pub use lock::IngestLock;
pub use purge::SPOOL_IDLE_PURGE_AFTER;

/// `meta` key: when the last [`catch_up`] finished (µs since the epoch).
pub const META_LAST_INGEST_AT: &str = "last_ingest_at_us";

/// `meta` key: the [`DERIVATION_VERSION`] the derived tables were built
/// with.
pub const META_DERIVATION_VERSION: &str = "derivation_version";

/// What ingest derives from its inputs, versioned. Bump it when a release
/// derives more (or differently) from input already read — transcripts read
/// to their end are never read again — so that [`catch_up`] rebuilds
/// existing archives once, as `claudit reingest` would.
///
/// - 1: tool calls read from transcripts (migration 0011).
/// - 2: subagent start / stop events (migration 0017), and the text of
///   system-injected prompts (a subagent's hand-back) from transcripts.
/// - 3: the leading command of a Bash call skips setup commands (`export`,
///   `echo`, `sleep`, …) and shell keywords (`for`, `until`, `if`), see
///   [`crate::shell`].
pub const DERIVATION_VERSION: u32 = 3;

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
    /// The derived tables were rebuilt first, because an older claudit had
    /// built them (see [`DERIVATION_VERSION`]).
    pub rebuilt: bool,
}

impl IngestReport {
    fn absorb(&mut self, other: IngestReport) {
        self.events += other.events;
        self.skipped_lines += other.skipped_lines;
        self.unprojected_events += other.unprojected_events;
        self.transcript_lines += other.transcript_lines;
        self.skipped_transcript_lines += other.skipped_transcript_lines;
        self.rebuilt |= other.rebuilt;
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
    store_derivation_version(&conn)?;
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
/// single-writer lock (or, if another run holds it, marks input as pending
/// for that run and returns at once), then runs passes until one finds no
/// new input and purges the spool files no longer needed. After releasing
/// the lock it runs again if a competing run marked input pending in the
/// meantime, so nothing spooled just before the release waits for the next
/// `Stop`.
pub fn catch_up(paths: &Paths, clock: &dyn Clock) -> Result<IngestOutcome> {
    let mut total: Option<IngestReport> = None;
    for _ in 0..MAX_PASSES {
        let Some(held) = IngestLock::try_acquire(paths)? else {
            lock::mark_pending(paths)?;
            break;
        };
        // Input marked before this point is covered by the passes below.
        lock::take_pending(paths)?;
        let round = catch_up_locked(paths, clock)?;
        total.get_or_insert_default().absorb(round);
        drop(held);
        if !lock::take_pending(paths)? {
            break;
        }
    }
    Ok(total.map_or(IngestOutcome::AlreadyRunning, IngestOutcome::Ran))
}

/// One locked round of [`catch_up`]: the one-time rebuild after an upgrade
/// if due, passes until idle, purge, bookkeeping.
fn catch_up_locked(paths: &Paths, clock: &dyn Clock) -> Result<IngestReport> {
    let mut total = IngestReport::default();
    {
        let mut conn = db::open(paths)?;
        if let Some(built_with) = outdated_derivation(&conn)? {
            logfile::info(
                paths,
                "ingest",
                format!(
                    "archive derived by an older claudit (derivation version {}): \
                     rebuilding it once, as `claudit reingest` would (version {DERIVATION_VERSION})",
                    built_with.map_or_else(|| "none".to_owned(), |v| v.to_string())
                ),
            );
            let replay = reingest::reset_and_replay(&mut conn, paths)?;
            total.unprojected_events += replay.unprojected_events;
            total.rebuilt = true;
        }
    }
    for _ in 0..MAX_PASSES {
        let pass = run(paths)?;
        if pass == IngestReport::default() {
            break;
        }
        total.absorb(pass);
    }
    let conn = db::open(paths)?;
    purge::purge_spool(&conn, paths, clock)?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![
            META_LAST_INGEST_AT,
            crate::clock::to_micros(clock.now()).to_string()
        ],
    )?;
    store_derivation_version(&conn)?;
    Ok(total)
}

/// `Some(stored version)` when the archive holds data derived by an older
/// claudit (an older or missing [`DERIVATION_VERSION`]); `None` when it is
/// current or has nothing to rebuild yet.
fn outdated_derivation(conn: &Connection) -> Result<Option<Option<u32>>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [META_DERIVATION_VERSION],
            |row| row.get(0),
        )
        .optional()?;
    let stored = stored.and_then(|v| v.parse::<u32>().ok());
    if stored.is_some_and(|v| v >= DERIVATION_VERSION) {
        return Ok(None);
    }
    let has_data: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM raw_events) OR EXISTS (SELECT 1 FROM sessions)",
        [],
        |row| row.get(0),
    )?;
    Ok(has_data.then_some(stored))
}

fn store_derivation_version(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![META_DERIVATION_VERSION, DERIVATION_VERSION.to_string()],
    )?;
    Ok(())
}
