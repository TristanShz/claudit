//! `claudit hook`: the command Claude Code runs (async) on every hook event.
//!
//! It stamps the payload with its receive time and appends it to the
//! session's spool. It parses nothing beyond `session_id` and
//! `hook_event_name`, never touches the database, never writes to stdout and
//! never fails: every error goes to the claudit log.

use std::io::Read;
use std::panic::{self, AssertUnwindSafe};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::clock::Clock;
use crate::logfile;
use crate::paths::Paths;
use crate::spool::{self, SpoolRecord};

/// Handles one hook invocation. Infallible by design: Claude Code must never
/// see a claudit failure.
pub fn run(paths: &Paths, clock: &dyn Clock, stdin: impl Read) {
    // Stamp first, before any I/O, so the receive time is as tight as possible.
    let received_at = clock.now();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| capture(paths, received_at, stdin)));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(err)) => logfile::error(paths, "hook", format!("{err:#}")),
        Err(_) => logfile::error(paths, "hook", "panic while capturing hook payload"),
    }
}

fn capture(paths: &Paths, received_at: DateTime<Utc>, mut stdin: impl Read) -> Result<()> {
    let mut input = Vec::new();
    stdin
        .read_to_end(&mut input)
        .context("read hook payload from stdin")?;
    let payload = serde_json::from_slice(&input).context("hook payload is not valid JSON")?;
    let record = SpoolRecord {
        received_at,
        payload,
    };
    record.hook_event_name()?;
    spool::append(paths, &record)
}
