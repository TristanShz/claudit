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

    /// Feeds `tests/fixtures/hooks/<name>` as received at `at`.
    pub fn hook_fixture_at(&self, at: DateTime<Utc>, name: &str) {
        self.at(at).hook_fixture(name);
    }

    /// Replays the hook side of fixture session `8d0c5a3e-…` (the one whose
    /// transcripts sit under `tests/fixtures/transcripts`), with the receive
    /// times listed in `tests/fixtures/hooks/README.md`: turn 1 waits on a
    /// Bash permission prompt, turn 2 is a typed `/code-review` that runs a
    /// subagent making one Read call.
    pub fn replay_session_a_hooks(&self) {
        self.replay_session_a_hooks_with(|_, _| {});
    }

    /// [`Self::replay_session_a_hooks`], letting `edit` change each payload
    /// (given its fixture name) before it is fed.
    pub fn replay_session_a_hooks_with(&self, edit: impl Fn(&str, &mut Value)) {
        let ms = |ms: i64| t0() + Duration::milliseconds(ms);
        for (at, name) in [
            (-500, "session_start_startup.json"),
            (0, "user_prompt_submit.json"),
            (4_100, "pre_tool_use_bash.json"),
            (4_200, "permission_request_bash.json"),
            (10_000, "notification_permission_prompt.json"),
            (19_900, "post_tool_use_bash_after_prompt.json"),
            (30_300, "stop_first_turn.json"),
            (299_990, "user_prompt_expansion_skill.json"),
            (300_000, "user_prompt_submit_skill.json"),
            (302_100, "pre_tool_use_agent.json"),
            (302_200, "subagent_start.json"),
            (306_100, "pre_tool_use_subagent_read.json"),
            (306_900, "post_tool_use_subagent_read.json"),
            (365_500, "subagent_stop.json"),
            (365_600, "post_tool_use_agent_review.json"),
            (380_300, "stop_after_subagent.json"),
            (1_800_000, "session_end_exit.json"),
        ] {
            self.at(ms(at))
                .hook_fixture_with(name, |payload| edit(name, payload));
        }
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

    /// Copies `tests/fixtures/transcripts/projects/<relative>` to the same
    /// place under the Claude projects dir.
    pub fn drop_transcript_fixture(&self, relative: &str) -> PathBuf {
        self.drop_transcript(relative, &transcript_fixture(relative))
    }

    /// Copies the whole fixture projects tree (every session, subagents
    /// included) into the Claude projects dir.
    pub fn drop_projects_fixture(&self) {
        copy_tree(
            &fixtures_dir().join("transcripts/projects"),
            &self.paths.claude_projects_dir(),
        );
    }

    /// The fixture archive the dashboard checks and the demo archive use:
    /// every fixture transcript, session `8d0c5a3e-…`'s hooks, and a Read
    /// call running in parallel with that session's first Bash call.
    pub fn populate_fixture_archive(&self) {
        const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";
        self.drop_projects_fixture();
        self.replay_session_a_hooks();
        let parallel = |p: &mut Value| {
            p["session_id"] = serde_json::json!(SESSION_A);
            p["prompt_id"] = serde_json::json!("a1b2c3d4-0001-4000-8000-000000000001");
            p["tool_name"] = serde_json::json!("Read");
            p["tool_use_id"] = serde_json::json!("toolu_01AcmeParallelRead");
            p["tool_input"] =
                serde_json::json!({ "file_path": "/Users/alice/code/acme-api/Cargo.toml" });
        };
        self.at(t0() + Duration::milliseconds(18_000))
            .hook_fixture_with("pre_tool_use_bash.json", parallel);
        self.at(t0() + Duration::milliseconds(22_000))
            .hook_fixture_with("post_tool_use_read.json", |p| {
                parallel(p);
                p["duration_ms"] = serde_json::json!(4000);
            });
    }

    /// Copies the tool-call fixture session
    /// (`tests/fixtures/transcripts/tool_calls`, a backfilled session with
    /// Bash, Read and Agent calls and a subagent) into the Claude projects
    /// dir.
    pub fn drop_tool_calls_fixture(&self) {
        copy_tree(
            &fixtures_dir().join("transcripts/tool_calls"),
            &self.paths.claude_projects_dir(),
        );
    }

    /// Replays the hook payloads captured from a real Claude Code session
    /// (`tests/fixtures/hooks/captured-<version>/`), in file-name order, each
    /// at the receive time its `received_at.json` records.
    pub fn replay_captured_hooks(&self, version: &str) {
        self.replay_hook_dir(&format!("captured-{version}"));
    }

    /// Replays the synthetic session `9f4c2b7a-…`
    /// (`tests/fixtures/hooks/activities/`): tests, builds, git, edits and
    /// one other shell command on 2026-03-05, known only from hooks.
    pub fn replay_activities_session(&self) {
        self.replay_hook_dir("activities");
    }

    /// Feeds every payload of `tests/fixtures/hooks/<dir>/`, in file-name
    /// order, at the receive time its `received_at.json` records.
    fn replay_hook_dir(&self, dir: &str) {
        let path = fixtures_dir().join("hooks").join(dir);
        let times: std::collections::BTreeMap<String, DateTime<Utc>> =
            serde_json::from_str(&fs::read_to_string(path.join("received_at.json")).unwrap())
                .unwrap();
        for (name, at) in times {
            self.hook_fixture_at(at, &format!("{dir}/{name}"));
        }
    }

    /// Copies the transcripts of that captured session
    /// (`tests/fixtures/transcripts/captured-<version>/projects`) into the
    /// Claude projects dir.
    pub fn drop_captured_transcripts(&self, version: &str) {
        copy_tree(
            &fixtures_dir().join(format!("transcripts/captured-{version}/projects")),
            &self.paths.claude_projects_dir(),
        );
    }

    // ---- ingest & stats ---------------------------------------------------

    /// Runs the locked catch-up ingest (what `claudit ingest` and
    /// `claudit serve` run), requires it to have taken the lock, and returns
    /// its report.
    ///
    /// Retries briefly on `AlreadyRunning`: while another test thread forks
    /// a process, the child briefly shares every open descriptor of this
    /// process, including a just-released ingest lock.
    pub fn ingest(&self) -> claudit::ingest::IngestReport {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match self.try_ingest() {
                claudit::ingest::IngestOutcome::Ran(report) => return report,
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

    /// Runs ingest and returns what it reports (counts of this run only).
    pub fn ingest_report(&self) -> claudit::ingest::IngestReport {
        claudit::ingest::run(&self.paths).expect("ingest succeeds")
    }

    pub fn consumption(&self, filter: &Filter) -> stats::consumption::Consumption {
        stats::consumption::consumption(&self.db(), filter).expect("consumption")
    }

    pub fn sessions(&self, filter: &Filter) -> Vec<stats::sessions::SessionSummary> {
        stats::sessions::session_list(&self.db(), filter).expect("session_list")
    }

    pub fn ingest_status(&self) -> stats::ingest_status::IngestStatus {
        stats::ingest_status::ingest_status(&self.db()).expect("ingest_status")
    }

    pub fn time_breakdown(&self, filter: &Filter) -> stats::time::TimeBreakdown {
        stats::time::time_breakdown(&self.db(), filter).expect("time_breakdown")
    }

    pub fn turn_times(&self, filter: &Filter) -> Vec<stats::time::TurnTime> {
        stats::time::turn_times(&self.db(), filter).expect("turn_times")
    }

    pub fn waiting_by_tool(&self, filter: &Filter) -> Vec<stats::time::ToolWaiting> {
        stats::time::waiting_by_tool(&self.db(), filter).expect("waiting_by_tool")
    }

    pub fn skills(&self, filter: &Filter) -> Vec<stats::skills::SkillStat> {
        stats::skills::skill_ranking(&self.db(), filter).expect("skill_ranking")
    }

    pub fn subagents(&self, filter: &Filter) -> Vec<stats::subagents::SubagentTypeStat> {
        stats::subagents::subagent_ranking(&self.db(), filter).expect("subagent_ranking")
    }

    pub fn subagent_runs(&self, filter: &Filter) -> Vec<stats::subagents::SubagentRun> {
        stats::subagents::subagent_runs(&self.db(), filter).expect("subagent_runs")
    }

    /// A connection to the archive, for calling any `claudit::stats` report.
    pub fn db(&self) -> rusqlite::Connection {
        claudit::db::open(&self.paths).expect("open database")
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

    /// Contents of the claudit error log ("" when absent).
    pub fn log(&self) -> String {
        fs::read_to_string(self.paths.log_file()).unwrap_or_default()
    }
}

// Only integration tests have the binary's path (`examples/demo_archive.rs`
// includes this harness too).
#[cfg(test)]
impl TestEnv {
    /// Runs the real `claudit` binary with this env's `CLAUDIT_HOME` and
    /// `CLAUDE_CONFIG_DIR`, feeding `stdin`.
    pub fn run_bin(&self, args: &[&str], stdin: &[u8]) -> std::process::Output {
        use std::process::{Command, Stdio};
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
}

/// Loads `tests/fixtures/hooks/<name>` as JSON.
pub fn hook_fixture(name: &str) -> Value {
    let path = fixtures_dir().join("hooks").join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Contents of `tests/fixtures/transcripts/<relative>`.
pub fn transcripts_fixture_file(relative: &str) -> String {
    let path = fixtures_dir().join("transcripts").join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Contents of `tests/fixtures/transcripts/projects/<relative>`.
pub fn transcript_fixture(relative: &str) -> String {
    let path = fixtures_dir().join("transcripts/projects").join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

pub fn fixtures_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}
