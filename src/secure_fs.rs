//! Owner-only filesystem helpers: every claudit directory is created 0700 and
//! every claudit file 0600, so other local users cannot read the archive.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Mode for every directory claudit creates.
pub const DIR_MODE: u32 = 0o700;
/// Mode for every file claudit creates.
pub const FILE_MODE: u32 = 0o600;

/// Creates `dir` and any missing parents with mode 0700.
pub fn create_dir_all(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)
}

/// Opens `path` for appending, creating it (and its parent) owner-only.
pub fn open_append(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(FILE_MODE)
        .open(path)
}

/// Restricts an existing file to owner-only access (e.g. files created by
/// SQLite, which does not take a mode).
pub fn restrict_file(path: &Path) -> io::Result<()> {
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE))?;
    }
    Ok(())
}
