//! Spool purge: once a spool file's content is safely in `raw_events`, the
//! file is deleted if its session has ended, or once it has been idle for
//! [`SPOOL_IDLE_PURGE_AFTER`] (sessions that crashed never send
//! `SessionEnd`). Runs only under the ingest lock.
//!
//! A hook that opened the file just before its removal would append to the
//! unlinked file and that event would be lost; restricting the purge to
//! ended or day-idle sessions makes that window practically unreachable.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use super::offsets::{self, FileId};
use crate::clock::{self, Clock};
use crate::paths::Paths;

/// How long after its last event a session's fully ingested spool is kept
/// when the session never ended.
pub const SPOOL_IDLE_PURGE_AFTER: Duration = Duration::hours(24);

/// Deletes every spool file that is fully ingested and either ended or
/// idle. Returns how many were deleted.
pub(super) fn purge_spool(conn: &Connection, paths: &Paths, clock: &dyn Clock) -> Result<u64> {
    let mut purged = 0;
    for path in super::spool::spool_files(&paths.spool_dir())? {
        if is_purgeable(conn, &path, clock.now())? {
            delete(conn, &path).with_context(|| format!("purge {}", path.display()))?;
            purged += 1;
        }
    }
    Ok(purged)
}

fn is_purgeable(conn: &Connection, path: &Path, now: DateTime<Utc>) -> Result<bool> {
    let file = FileId::of(path)?;
    let metadata = std::fs::metadata(path)?;
    if offsets::get(conn, &file)? < metadata.len() {
        return Ok(false); // unread input (possibly a line being written)
    }
    let Some(session_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(false);
    };
    // The session's latest archived event: a session resumed after its
    // `SessionEnd` is live again, so only a final `SessionEnd` counts.
    let last: Option<(String, i64)> = conn
        .query_row(
            "SELECT hook_event_name, received_at_us FROM raw_events
             WHERE session_id = ?1
             ORDER BY received_at_us DESC, id DESC
             LIMIT 1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let last_activity = match &last {
        Some((event, _)) if event == "SessionEnd" => return Ok(true),
        Some((_, received_at_us)) => clock::from_micros(*received_at_us),
        // Nothing archivable in the file (only skipped lines): its own
        // modification time is the only activity there is.
        None => DateTime::<Utc>::from(metadata.modified()?),
    };
    Ok(now - last_activity >= SPOOL_IDLE_PURGE_AFTER)
}

/// Removes the file, then forgets its offsets, so a spool recreated at the
/// same path (a resumed session) is read from its start even if the file
/// system reuses the inode.
fn delete(conn: &Connection, path: &Path) -> Result<()> {
    std::fs::remove_file(path)?;
    offsets::forget(conn, &path.to_string_lossy())?;
    Ok(())
}
