//! The single-writer ingest lock: an advisory `flock` on
//! `$CLAUDIT_HOME/ingest.lock`. The kernel releases it when the holder's file
//! is closed, including when the holder crashes, so it can never go stale.

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
