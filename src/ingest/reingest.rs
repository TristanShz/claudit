//! `claudit reingest`: rebuilds every derived table from the archive.
//!
//! Sources are `raw_events` (hook payloads, kept forever) and whatever
//! transcripts are still on disk. The rebuild:
//! 1. re-sanitizes every archived payload with the current redaction
//!    patterns (so a pattern added later applies retroactively);
//! 2. clears every table of [`DERIVED_TABLES`], the derived `meta` keys and
//!    the transcript offsets (spool offsets are kept: spool lines already
//!    archived must not be archived twice);
//! 3. replays `raw_events` in archive order through the same projection as
//!    ingest ([`events::project`]);
//!
//! all in one transaction, then runs a normal ingest, which reads the
//! transcripts from the start and picks up any new spool lines.
//!
//! **Extension point:** a migration that adds a table must also add it to
//! [`DERIVED_TABLES`] (rebuilt) or [`KEPT_TABLES`] (left alone); a test fails
//! while a table is in neither.

use anyhow::Result;
use rusqlite::{Connection, TransactionBehavior, params};

use super::events::{self, Projection, RawEvent};
use super::transcripts;
use crate::clock;
use crate::logfile;
use crate::paths::Paths;
use crate::redact;

/// Tables derived from `raw_events` and transcripts, cleared and rebuilt by
/// reingest. List child tables before the tables they reference.
pub const DERIVED_TABLES: &[&str] = &[
    "permission_requests",
    "notifications",
    "skill_invocations",
    "subagent_events",
    "subagent_runs",
    "tool_calls",
    "sessions",
    "turns",
    "api_messages",
    "transcript_entries",
];

/// Tables reingest keeps: the replay source, schema bookkeeping and input
/// offsets (reset selectively, see the module docs).
pub const KEPT_TABLES: &[&str] = &["meta", "schema_migrations", "raw_events", "ingest_offsets"];

/// `meta` keys derived from the inputs, deleted before the rebuild.
const DERIVED_META_KEYS: &[&str] = &[transcripts::META_SKIPPED_LINES];

/// Archived events read per batch while replaying.
const BATCH: i64 = 1000;

/// What the replay of `raw_events` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replay {
    /// Archived events replayed.
    pub events: u64,
    /// Archived payloads changed by re-sanitizing them.
    pub resanitized: u64,
    /// Events whose payload could not be projected (logged).
    pub unprojected_events: u64,
}

/// Clears the derived state and replays `raw_events` into it (steps 1–3).
pub fn reset_and_replay(conn: &mut Connection, paths: &Paths) -> Result<Replay> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for table in DERIVED_TABLES {
        tx.execute(&format!("DELETE FROM \"{table}\""), [])?;
    }
    for key in DERIVED_META_KEYS {
        tx.execute("DELETE FROM meta WHERE key = ?1", [key])?;
    }
    reset_transcript_offsets(&tx, paths)?;

    let mut replay = Replay::default();
    let mut last_id = 0_i64;
    loop {
        let rows = archived_after(&tx, last_id)?;
        let Some(&(id, ..)) = rows.last() else { break };
        last_id = id;
        for (id, session_id, hook_event_name, received_at_us, payload) in rows {
            let stored: serde_json::Value = match serde_json::from_str(&payload) {
                Ok(value) => value,
                Err(err) => {
                    replay.unprojected_events += 1;
                    logfile::error(paths, "reingest", format!("raw event {id}: {err}"));
                    continue;
                }
            };
            let clean = redact::sanitize_hook_payload(stored.clone());
            if clean != stored {
                tx.execute(
                    "UPDATE raw_events SET payload = ?1 WHERE id = ?2",
                    params![clean.to_string(), id],
                )?;
                replay.resanitized += 1;
            }
            let event = RawEvent {
                session_id,
                hook_event_name,
                received_at: clock::from_micros(received_at_us),
                payload: clean,
            };
            replay.events += 1;
            if let Projection::Malformed(reason) = events::project(&tx, &event)? {
                replay.unprojected_events += 1;
                logfile::error(
                    paths,
                    "reingest",
                    format!("{} event not projected: {reason}", event.hook_event_name),
                );
            }
        }
    }
    tx.commit()?;
    Ok(replay)
}

type ArchivedRow = (i64, String, String, i64, String);

fn archived_after(conn: &Connection, after_id: i64) -> Result<Vec<ArchivedRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, session_id, hook_event_name, received_at_us, payload
         FROM raw_events WHERE id > ?1 ORDER BY id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![after_id, BATCH], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Forgets how far every non-spool file (i.e. every transcript) was read.
fn reset_transcript_offsets(conn: &Connection, paths: &Paths) -> Result<()> {
    let spool_dir = paths.spool_dir();
    let tracked: Vec<String> = conn
        .prepare("SELECT DISTINCT path FROM ingest_offsets")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    for path in tracked {
        if !std::path::Path::new(&path).starts_with(&spool_dir) {
            conn.execute("DELETE FROM ingest_offsets WHERE path = ?1", [&path])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// Guards the extension point: every table a migration creates is either
    /// rebuilt or deliberately kept by reingest.
    #[test]
    fn every_table_is_classified_for_reingest() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::new(root.path().join("home"), root.path().join("claude"));
        let conn = db::open(&paths).unwrap();
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for table in &tables {
            assert!(
                DERIVED_TABLES.contains(&table.as_str()) || KEPT_TABLES.contains(&table.as_str()),
                "table `{table}` must be listed in reingest::DERIVED_TABLES or KEPT_TABLES"
            );
        }
        for listed in DERIVED_TABLES.iter().chain(KEPT_TABLES) {
            assert!(
                tables.iter().any(|t| t == listed),
                "`{listed}` does not exist"
            );
        }
    }
}
