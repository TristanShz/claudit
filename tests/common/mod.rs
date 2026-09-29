//! Seam-1 test harness, shared by every feature test file.
//!
//! Seams under test (tests live only at these):
//! 1. The core library end to end: hook payloads go in through
//!    `claudit::hook::run` (the entry point `claudit hook` uses), transcripts
//!    are dropped into the Claude config dir, `claudit::ingest::run` loads
//!    them, and assertions are made only on typed `claudit::stats` reports,
//!    never on table layout, offsets or internal helpers.
//! 2. The `claudit hook` process contract (exit code, stdout, error log),
//!    exercised through the built binary.
//!
//! Each `TestEnv` owns an isolated temp `CLAUDIT_HOME`, a temp Claude config
//! dir and a manual clock, so tests never touch the real `~/.claudit` or
//! `~/.claude`.

// Each test binary uses a different subset of the harness.
#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Duration, TimeZone, Utc};
use claudit::clock::ManualClock;
use claudit::paths::Paths;
use claudit::stats::{self, Filter};
use serde_json::Value;
use tempfile::TempDir;

/// The instant every `TestEnv` clock starts at: 2026-03-02 09:00:00 UTC.
pub fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, 2, 9, 0, 0).unwrap()
}

pub struct TestEnv {
    // Held for its Drop: the temp tree is removed when the env goes away.
    _root: TempDir,
    pub paths: Paths,
    pub clock: ManualClock,
    /// Records the detached ingests the hook asks for instead of running them.
    pub spawner: RecordingSpawner,
}

/// An ingest spawner that only counts the spawns requested.
#[derive(Debug, Default)]
pub struct RecordingSpawner {
    spawns: AtomicUsize,
}

impl claudit::hook::IngestSpawner for RecordingSpawner {
    fn spawn_ingest(&self) -> anyhow::Result<()> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl TestEnv {
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("create temp dir");
        let paths = Paths::new(root.path().join("claudit-home"), root.path().join("claude"));
        fs::create_dir_all(paths.claude_projects_dir()).expect("create claude projects dir");
        Self {
            _root: root,
            paths,
            clock: ManualClock::new(t0()),
            spawner: RecordingSpawner::default(),
        }
    }

    // ---- time -------------------------------------------------------------

    /// Sets the clock used to stamp the next hook payloads.
    pub fn at(&self, at: DateTime<Utc>) -> &Self {
        self.clock.set(at);
        self
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) -> &Self {
        self.clock.advance(by);
        self
    }

    // ---- hook payloads ----------------------------------------------------

    /// Feeds raw bytes to the hook entry point, as Claude Code does on stdin.
    pub fn hook_raw(&self, stdin: &[u8]) {
        claudit::hook::run(&self.paths, &self.clock, &self.spawner, stdin);
    }

    /// How many detached ingests the hook has asked for so far.
    pub fn ingest_spawns(&self) -> usize {
        self.spawner.spawns.load(Ordering::SeqCst)
    }

    /// Feeds a JSON payload to the hook entry point.
    pub fn hook(&self, payload: &Value) {
        self.hook_raw(payload.to_string().as_bytes());
    }

    /// Feeds `tests/fixtures/hooks/<name>` to the hook entry point.
    pub fn hook_fixture(&self, name: &str) {
        self.hook(&hook_fixture(name));
    }

    /// Feeds a fixture after letting `edit` override fields
    /// (e.g. `|p| p["tool_use_id"] = json!("toolu_other")`).
    pub fn hook_fixture_with(&self, name: &str, edit: impl FnOnce(&mut Value)) {
        let mut payload = hook_fixture(name);
        edit(&mut payload);
        self.hook(&payload);
    }

    /// Session ids that currently have a spool file, sorted.
    pub fn spooled_sessions(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.paths.spool_dir()) else {
            return Vec::new();
        };
        let mut sessions: Vec<String> = entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        sessions.sort();
        sessions
    }

    // ---- transcripts ------------------------------------------------------

    /// Writes a file under the Claude projects dir (e.g.
    /// `"-Users-alice-code-acme-api/<session>.jsonl"`), replacing it.
    pub fn drop_transcript(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.paths.claude_projects_dir().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    /// Appends to a file under the Claude projects dir (for incremental
    /// ingestion and partial-line tests).
    pub fn append_transcript(&self, relative: &str, contents: &str) {
        let path = self.paths.claude_projects_dir().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }

    // ---- ingest & stats ---------------------------------------------------

    /// Runs the locked catch-up ingest (what `claudit ingest` and
    /// `claudit serve` run) and requires it to have taken the lock.
    ///
    /// Retries briefly on `AlreadyRunning`: while another test thread forks
    /// a process, the child briefly shares every open descriptor of this
    /// process, including a just-released ingest lock.
    pub fn ingest(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match self.try_ingest() {
                claudit::ingest::IngestOutcome::Ran(_) => return,
                claudit::ingest::IngestOutcome::AlreadyRunning => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "ingest lock never became free"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
    }

    /// Runs the locked catch-up ingest, which may find the lock taken.
    pub fn try_ingest(&self) -> claudit::ingest::IngestOutcome {
        claudit::ingest::catch_up(&self.paths, &self.clock).expect("ingest succeeds")
    }

    /// A connection to the archive, for calling any `claudit::stats` report.
    pub fn db(&self) -> rusqlite::Connection {
        claudit::db::open(&self.paths).expect("open database")
    }

    pub fn top_tools(&self, filter: &Filter) -> Vec<stats::tools::ToolStat> {
        stats::tools::top_tools(&self.db(), filter).expect("top_tools")
    }

    pub fn tool_ranking(&self, filter: &Filter) -> Vec<stats::tools::RankedCalls> {
        stats::tools::tool_ranking(&self.db(), filter).expect("tool_ranking")
    }

    pub fn bash_command_ranking(&self, filter: &Filter) -> Vec<stats::tools::RankedCalls> {
        stats::tools::bash_command_ranking(&self.db(), filter).expect("bash_command_ranking")
    }

    pub fn mcp_server_ranking(&self, filter: &Filter) -> Vec<stats::tools::RankedCalls> {
        stats::tools::mcp_server_ranking(&self.db(), filter).expect("mcp_server_ranking")
    }

    // ---- the binary -------------------------------------------------------

    /// Runs the real `claudit` binary with this env's `CLAUDIT_HOME` and
    /// `CLAUDE_CONFIG_DIR`, feeding `stdin`.
    pub fn run_bin(&self, args: &[&str], stdin: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_claudit"))
            .args(args)
            .env("CLAUDIT_HOME", self.paths.home())
            .env("CLAUDE_CONFIG_DIR", self.paths.claude_config_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn claudit");
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().expect("wait for claudit")
    }

    /// Contents of the claudit error log ("" when absent).
    pub fn log(&self) -> String {
        fs::read_to_string(self.paths.log_file()).unwrap_or_default()
    }
}

/// Loads `tests/fixtures/hooks/<name>` as JSON.
pub fn hook_fixture(name: &str) -> Value {
    let path = fixtures_dir().join("hooks").join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn fixtures_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}
