//! Owner-only permissions (#7): every directory claudit creates is 0700 and
//! every file 0600.
//!
//! Seam under test: seam 1 (hook payloads in through `claudit::hook::run`,
//! `claudit::ingest::run`), observing the modes of the files those entry
//! points leave under `CLAUDIT_HOME`. Unix only: Windows files inherit the
//! user profile's ACL.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::TestEnv;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

#[test]
fn every_claudit_directory_is_owner_only() {
    let env = TestEnv::new();
    env.hook_fixture("post_tool_use_bash.json");
    env.hook_raw(b"not json"); // logged as an error
    env.ingest();

    for dir in [
        env.paths.home().to_path_buf(),
        env.paths.spool_dir(),
        env.paths.logs_dir(),
    ] {
        assert_eq!(mode(&dir), 0o700, "{}", dir.display());
    }
}

#[test]
fn every_claudit_file_is_owner_only() {
    let env = TestEnv::new();
    env.hook_fixture("post_tool_use_bash.json");
    env.hook_raw(b"not json"); // logged as an error
    env.ingest();
    // An open connection keeps the WAL side files on disk.
    let _conn = env.db();

    let db = env.paths.db_path();
    let spool_file = std::fs::read_dir(env.paths.spool_dir())
        .unwrap()
        .next()
        .expect("a spool file")
        .unwrap()
        .path();
    for file in [
        spool_file,
        db.clone(),
        with_suffix(&db, "-wal"),
        with_suffix(&db, "-shm"),
        env.paths.log_file(),
    ] {
        assert_eq!(mode(&file), 0o600, "{}", file.display());
    }
}
