//! The single-writer ingest lock: a file lock ([`File::try_lock`]: `flock`
//! on Unix, `LockFileEx` on Windows) on `$CLAUDIT_HOME/ingest.lock`. The
//! kernel releases it when the holder's file is closed, including when the
//! holder crashes, so it can never go stale.
//!
//! A run that finds the lock taken leaves a pending marker
//! (`$CLAUDIT_HOME/ingest.pending`) before exiting; the holder checks for it
//! after releasing the lock, which closes the gap between the holder's last
//! pass and its release.

use std::fs::{File, TryLockError};
use std::io;

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
        let file = secure_fs::open_lock(&path)
            .with_context(|| format!("open lock file {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(err)) => {
                Err(err).with_context(|| format!("lock {}", path.display()))
            }
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
