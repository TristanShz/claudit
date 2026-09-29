//! The spool: one append-only JSONL file per session under
//! `$CLAUDIT_HOME/spool/<session_id>.jsonl`, written by the hook and read by
//! ingest. Each line is a [`SpoolRecord`].

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths::Paths;
use crate::secure_fs;

/// One spooled hook event: the untouched payload plus its receive time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpoolRecord {
    /// When the hook process received the payload (nanosecond RFC 3339).
    pub received_at: DateTime<Utc>,
    /// The hook payload exactly as Claude Code sent it (re-serialized compact).
    pub payload: Value,
}

impl SpoolRecord {
    /// The payload's `session_id`, validated to be safe as a file name.
    pub fn session_id(&self) -> Result<&str> {
        let id = self
            .payload
            .get("session_id")
            .and_then(Value::as_str)
            .context("hook payload has no string session_id")?;
        let valid = !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !valid {
            bail!("hook payload session_id {id:?} is not a valid identifier");
        }
        Ok(id)
    }

    /// The payload's `hook_event_name` (e.g. `PostToolUse`).
    pub fn hook_event_name(&self) -> Result<&str> {
        self.payload
            .get("hook_event_name")
            .and_then(Value::as_str)
            .context("hook payload has no string hook_event_name")
    }
}

/// The spool file of a session.
pub fn file_for(paths: &Paths, session_id: &str) -> PathBuf {
    paths.spool_dir().join(format!("{session_id}.jsonl"))
}

/// Appends `record` to its session's spool file as a single `write` of one
/// complete line, so concurrent hooks of the same session never interleave.
pub fn append(paths: &Paths, record: &SpoolRecord) -> Result<()> {
    let session_id = record.session_id()?;
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    let path = file_for(paths, session_id);
    let mut file = secure_fs::open_append(&path)
        .with_context(|| format!("open spool file {}", path.display()))?;
    file.write_all(&line)
        .with_context(|| format!("append to spool file {}", path.display()))?;
    Ok(())
}
