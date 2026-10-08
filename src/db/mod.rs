//! The SQLite archive: opening it (WAL, busy timeout, owner-only files) and
//! bringing its schema up to date.

mod migrations;

use std::fs::OpenOptions;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::paths::Paths;
use crate::secure_fs;

pub use migrations::schema_version;

/// How long a connection waits on a lock held by another process.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// Opens (creating if needed) the archive and applies pending migrations.
pub fn open(paths: &Paths) -> Result<Connection> {
    secure_fs::create_dir_all(paths.home())
        .with_context(|| format!("create {}", paths.home().display()))?;
    let path = paths.db_path();
    // Create the file ourselves so it is owner-only from its first byte.
    secure_fs::owner_only(OpenOptions::new().create(true).append(true))
        .open(&path)
        .with_context(|| format!("create database {}", path.display()))?;

    let mut conn =
        Connection::open(&path).with_context(|| format!("open database {}", path.display()))?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    for suffix in ["-wal", "-shm"] {
        let mut side = path.clone().into_os_string();
        side.push(suffix);
        secure_fs::restrict_file(side.as_ref())?;
    }

    migrations::migrate(&mut conn)?;
    Ok(conn)
}
