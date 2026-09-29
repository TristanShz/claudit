//! claudit: local analytics for Claude Code sessions.
//!
//! Data flow: `hook` appends raw hook payloads to a per-session spool;
//! `ingest` loads the spool (and, later, transcripts) into SQLite; `stats`
//! answers typed queries over the archive; `web` renders them.

pub mod clock;
pub mod db;
pub mod hook;
pub mod ingest;
pub mod install;
pub mod logfile;
pub mod paths;
pub mod pricing;
pub mod secure_fs;
pub mod spool;
pub mod stats;
pub mod web;
