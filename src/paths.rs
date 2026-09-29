//! Where claudit keeps its data, and where Claude Code keeps its own.
//!
//! `CLAUDIT_HOME` (default `~/.claudit`) holds the spool, the database, logs
//! and state files. `CLAUDE_CONFIG_DIR` (default `~/.claude`) is Claude Code's
//! configuration directory, holding `settings.json` and `projects/`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Resolved locations of every file and directory claudit touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: PathBuf,
    claude_config_dir: PathBuf,
}

impl Paths {
    /// Builds paths from explicit roots (used by tests and by `from_env`).
    pub fn new(home: impl Into<PathBuf>, claude_config_dir: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            claude_config_dir: claude_config_dir.into(),
        }
    }

    /// Resolves paths from `CLAUDIT_HOME`, `CLAUDE_CONFIG_DIR` and `HOME`.
    pub fn from_env() -> Result<Self> {
        let user_home = || -> Result<PathBuf> {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .context("HOME is not set")
        };
        let home = match non_empty_var("CLAUDIT_HOME") {
            Some(dir) => dir,
            None => user_home()?.join(".claudit"),
        };
        let claude_config_dir = match non_empty_var("CLAUDE_CONFIG_DIR") {
            Some(dir) => dir,
            None => user_home()?.join(".claude"),
        };
        Ok(Self::new(home, claude_config_dir))
    }

    /// `CLAUDIT_HOME` itself.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Directory of per-session spool files written by the hook.
    pub fn spool_dir(&self) -> PathBuf {
        self.home.join("spool")
    }

    /// The SQLite archive.
    pub fn db_path(&self) -> PathBuf {
        self.home.join("claudit.db")
    }

    /// Directory of claudit log files.
    pub fn logs_dir(&self) -> PathBuf {
        self.home.join("logs")
    }

    /// The error log every internal failure is appended to.
    pub fn log_file(&self) -> PathBuf {
        self.logs_dir().join("claudit.log")
    }

    /// What `claudit install` changed and `claudit uninstall` must restore.
    pub fn install_state_file(&self) -> PathBuf {
        self.home.join("install-state.json")
    }

    /// Claude Code's configuration directory.
    pub fn claude_config_dir(&self) -> &Path {
        &self.claude_config_dir
    }

    /// Claude Code's user settings file.
    pub fn claude_settings_file(&self) -> PathBuf {
        self.claude_config_dir.join("settings.json")
    }

    /// Claude Code's transcripts root (`<config>/projects`).
    pub fn claude_projects_dir(&self) -> PathBuf {
        self.claude_config_dir.join("projects")
    }
}

fn non_empty_var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}
