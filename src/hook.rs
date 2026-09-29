//! `claudit hook`: the command Claude Code runs (async) on every hook event.
//!
//! It stamps the payload with its receive time and appends it to the
//! session's spool. It parses nothing beyond `session_id` and
//! `hook_event_name`, never touches the database, never writes to stdout and
//! never fails: every error goes to the claudit log. Stdin that is not valid
//! JSON is logged and still spooled, raw, to `spool/unparsed.jsonl`, so
//! nothing Claude Code sent is lost.
//!
//! On `Stop` and `SessionEnd` it also asks its [`IngestSpawner`] to start
//! `claudit ingest` in the background, and returns without waiting for it.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::panic::{self, AssertUnwindSafe};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::clock::Clock;
use crate::logfile;
use crate::paths::Paths;
use crate::spool::{self, SpoolRecord};

/// Hook events after which new data is worth ingesting: the end of a turn
/// and the end of a session.
const INGEST_TRIGGERS: [&str; 2] = ["Stop", "SessionEnd"];

/// Starts `claudit ingest` without waiting for it. Injected so tests can
/// observe the request without running a process.
pub trait IngestSpawner {
    fn spawn_ingest(&self) -> Result<()>;
}

/// The production spawner: runs `<this executable> ingest` in a new session
/// (so it outlives Claude Code and its terminal), with stdin, stdout and
/// stderr on `/dev/null` (so Claude Code never waits on inherited pipes),
/// and never waits for it: the orphan is reaped by init.
#[derive(Debug, Default, Clone, Copy)]
pub struct DetachedIngest;

impl IngestSpawner for DetachedIngest {
    fn spawn_ingest(&self) -> Result<()> {
        let exe = std::env::current_exe().context("locate the claudit executable")?;
        let mut command = Command::new(exe);
        command
            .arg("ingest")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: `setsid` is async-signal-safe, as required between fork
        // and exec; it only fails if the child already leads a process
        // group, which a freshly forked child never does.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn().context("spawn claudit ingest")?;
        Ok(())
    }
}

/// Handles one hook invocation. Infallible by design: Claude Code must never
/// see a claudit failure.
pub fn run(paths: &Paths, clock: &dyn Clock, spawner: &dyn IngestSpawner, stdin: impl Read) {
    // Stamp first, before any I/O, so the receive time is as tight as possible.
    let received_at = clock.now();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let event = capture(paths, received_at, stdin)?;
        if INGEST_TRIGGERS.contains(&event.as_str()) {
            spawner.spawn_ingest().context("spawn detached ingest")?;
        }
        Ok::<_, anyhow::Error>(())
    }));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(err)) => logfile::error(paths, "hook", format!("{err:#}")),
        Err(_) => logfile::error(paths, "hook", "panic while capturing hook payload"),
    }
}

/// Spools the payload and returns its `hook_event_name`.
fn capture(paths: &Paths, received_at: DateTime<Utc>, mut stdin: impl Read) -> Result<String> {
    let mut input = Vec::new();
    stdin
        .read_to_end(&mut input)
        .context("read hook payload from stdin")?;
    let record = match serde_json::from_slice(&input) {
        Ok(payload) => SpoolRecord {
            received_at,
            payload,
        },
        Err(err) => {
            logfile::error(
                paths,
                "hook",
                format!("hook payload is not valid JSON ({err}); spooled raw"),
            );
            SpoolRecord::unparsed(received_at, &input)
        }
    };
    let event = record.hook_event_name()?.to_owned();
    spool::append(paths, &record)?;
    Ok(event)
}
