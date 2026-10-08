//! Owner-only filesystem helpers: on Unix every claudit directory is created
//! 0700 and every claudit file 0600, so other local users cannot read the
//! archive. On Windows, files inherit the ACL of the user's profile
//! directory, which other users already cannot read; the helpers then only
//! create.

use std::fs::{self, File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Mode for every directory claudit creates (Unix).
pub const DIR_MODE: u32 = 0o700;
/// Mode for every file claudit creates (Unix).
pub const FILE_MODE: u32 = 0o600;

/// Creates `dir` and any missing parents with mode 0700.
pub fn create_dir_all(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(DIR_MODE);
    builder.create(dir)
}

/// Makes `options` create its file owner-only (0600 on Unix).
pub fn owner_only(options: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    options.mode(FILE_MODE);
    options
}

/// Opens `path` for appending, creating it (and its parent) owner-only.
pub fn open_append(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    owner_only(OpenOptions::new().create(true).append(true)).open(path)
}

/// Opens `path` to hold a lock on it ([`File::try_lock`]), creating it (and
/// its parent) owner-only. Read and write access, as Windows requires to
/// lock a file.
pub fn open_lock(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    owner_only(
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false),
    )
    .open(path)
}

/// Restricts an existing file to owner-only access (e.g. files created by
/// SQLite, which does not take a mode).
pub fn restrict_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
