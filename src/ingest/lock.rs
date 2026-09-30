//! The single-writer ingest lock: an advisory `flock` on
//! `$CLAUDIT_HOME/ingest.lock`. The kernel releases it when the holder's file
//! is closed, including when the holder crashes, so it can never go stale.
//!
//! A run that finds the lock taken leaves a pending marker
//! (`$CLAUDIT_HOME/ingest.pending`) before exiting; the holder checks for it
//! after releasing the lock, which closes the gap between the holder's last
//! pass and its release.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

use anyhow::{Context, Result};

use crate::paths::Paths;
use crate::secure_fs;

/// Proof that this process is the only ingest writer. Released on drop.
#[derive(Debug)]
pub struct IngestLock {
    _file: File,
}

impl IngestLock {
    /// Takes the lock if it is free; `None` if another ingest holds it.
    /// Never blocks.
    pub fn try_acquire(paths: &Paths) -> Result<Option<Self>> {
        let path = paths.ingest_lock_file();
        let file = secure_fs::open_append(&path)
            .with_context(|| format!("open lock file {}", path.display()))?;
        // SAFETY: `flock` only reads the descriptor, which `file` keeps open.
        let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if taken == 0 {
            return Ok(Some(Self { _file: file }));
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(err).with_context(|| format!("lock {}", path.display()))
        }
    }
}

/// Records that an ingest was requested while the lock was taken.
pub(super) fn mark_pending(paths: &Paths) -> Result<()> {
    let path = paths.ingest_pending_file();
    secure_fs::open_append(&path)
        .map(drop)
        .with_context(|| format!("create {}", path.display()))
}

/// Consumes the pending marker: true if one was there.
pub(super) fn take_pending(paths: &Paths) -> Result<bool> {
    let path = paths.ingest_pending_file();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("remove {}", path.display())),
    }
}
