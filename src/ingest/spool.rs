//! Loads spool files into the archive: each new line is archived in
//! `raw_events` and projected into the normalized tables.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior};

use super::IngestReport;
use super::events::{self, Projection, RawEvent};
use super::offsets::{self, FileId};
use crate::logfile;
use crate::paths::Paths;
use crate::spool::SpoolRecord;

/// Ingests every spool file's new complete lines.
pub fn ingest(conn: &mut Connection, paths: &Paths) -> Result<IngestReport> {
    let mut report = IngestReport::default();
    for path in spool_files(&paths.spool_dir())? {
        let file_report = ingest_file(conn, paths, &path)
            .with_context(|| format!("ingest spool file {}", path.display()))?;
        report.absorb(file_report);
    }
    Ok(report)
}

/// The spool files, oldest name first (deterministic order).
pub(super) fn spool_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn ingest_file(conn: &mut Connection, paths: &Paths, path: &Path) -> Result<IngestReport> {
    let file = FileId::of(path)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let start = offsets::get(&tx, &file)?;
    let chunk = offsets::read_complete_lines(path, start)?;

    let mut report = IngestReport::default();
    for line in chunk.lines.iter().filter(|line| !line.trim().is_empty()) {
        let event = match parse_line(line) {
            Ok(event) => event,
            Err(err) => {
                report.skipped_lines += 1;
                logfile::error(
                    paths,
                    "ingest",
                    format!("skipped spool line in {}: {err:#}", path.display()),
                );
                continue;
            }
        };
        events::archive(&tx, &event)?;
        report.events += 1;
        if let Projection::Malformed(reason) = events::project(&tx, &event)? {
            report.unprojected_events += 1;
            logfile::error(
                paths,
                "ingest",
                format!("{} event not projected: {reason}", event.hook_event_name),
            );
        }
    }
    offsets::set(&tx, &file, chunk.end_offset)?;
    tx.commit()?;
    Ok(report)
}

fn parse_line(line: &str) -> Result<RawEvent> {
    let record: SpoolRecord = serde_json::from_str(line).context("invalid spool record")?;
    RawEvent::from_spool(record)
}
