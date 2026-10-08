//! Incremental reading of append-only line files (spool files, transcripts).
//!
//! Progress is a byte offset per file identity (path + inode, or file index
//! on Windows). Only complete lines are consumed: a trailing partial line
//! stays for the next run.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::clock::{self, Clock, SystemClock};

/// Identity of an input file: a file replaced at the same path (new inode)
/// is read from the start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileId {
    pub path: String,
    /// The inode on Unix, the NTFS file index on Windows.
    pub inode: u64,
}

impl FileId {
    pub fn of(path: &Path) -> io::Result<Self> {
        Ok(Self {
            path: path.to_string_lossy().into_owned(),
            inode: inode(path)?,
        })
    }
}

#[cfg(unix)]
fn inode(path: &Path) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(path)?.ino())
}

#[cfg(windows)]
fn inode(path: &Path) -> io::Result<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = File::open(path)?;
    // SAFETY: an all-zero `BY_HANDLE_FILE_INFORMATION` is valid (plain
    // integers), and the handle stays open for the duration of the call.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow))
}

/// The complete lines found past an offset, and the offset just after them.
#[derive(Debug, Clone, Default)]
pub struct NewLines {
    pub lines: Vec<String>,
    pub end_offset: u64,
}

/// The offset already ingested for `file` (0 if never seen).
pub fn get(conn: &Connection, file: &FileId) -> Result<u64> {
    let offset: Option<i64> = conn
        .query_row(
            "SELECT byte_offset FROM ingest_offsets WHERE path = ?1 AND inode = ?2",
            params![file.path, file.inode as i64],
            |row| row.get(0),
        )
        .optional()?;
    Ok(offset.unwrap_or(0) as u64)
}

/// Records that `file` has been ingested up to `offset`.
pub fn set(conn: &Connection, file: &FileId, offset: u64) -> Result<()> {
    conn.execute(
        "INSERT INTO ingest_offsets (path, inode, byte_offset, updated_at_us)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path, inode) DO UPDATE
         SET byte_offset = excluded.byte_offset, updated_at_us = excluded.updated_at_us",
        params![
            file.path,
            file.inode as i64,
            offset as i64,
            clock::to_micros(SystemClock.now())
        ],
    )?;
    Ok(())
}

/// Forgets every offset recorded for `path` (whatever its inode), once the
/// file is deleted.
pub fn forget(conn: &Connection, path: &str) -> Result<()> {
    conn.execute("DELETE FROM ingest_offsets WHERE path = ?1", params![path])?;
    Ok(())
}

/// Reads every complete line after `offset`. If the file is now shorter than
/// `offset` (truncated), it is read from the start.
pub fn read_complete_lines(path: &Path, offset: u64) -> io::Result<NewLines> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = if offset > len { 0 } else { offset };
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let Some(last_newline) = buf.iter().rposition(|&b| b == b'\n') else {
        return Ok(NewLines {
            lines: Vec::new(),
            end_offset: start,
        });
    };
    let lines = buf[..last_newline]
        .split(|&b| b == b'\n')
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect();
    Ok(NewLines {
        lines,
        end_offset: start + last_newline as u64 + 1,
    })
}
