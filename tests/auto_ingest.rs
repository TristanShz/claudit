//! Automatic ingestion (#4): the hook spawns a detached ingest on `Stop` and
//! `SessionEnd`, concurrent ingests coalesce behind a single-writer lock, and
//! spool files are purged once archived.
//!
//! Seams under test:
//! - seam 1 (library end to end): hook payloads in through the hook entry
//!   point with a recording ingest spawner, `TestEnv::ingest` (the locked
//!   catch-up `claudit ingest` and `claudit serve` run), typed stats out;
//!   spool purge is observed as the presence of the session's spool file,
//!   which is the user-visible contract of the purge;
//! - seam 2 (the `claudit hook` process): it returns without waiting for the
//!   ingest it spawned.

mod common;

use std::sync::Barrier;
use std::time::{Duration as StdDuration, Instant};

use chrono::Duration;
use claudit::ingest::{IngestLock, IngestOutcome};
use claudit::stats::Filter;
use common::{TestEnv, hook_fixture};
use serde_json::json;

#[test]
fn stop_and_session_end_request_an_ingest_other_events_do_not() {
    let env = TestEnv::new();

    env.hook_fixture("post_tool_use_bash.json");
    assert_eq!(env.ingest_spawns(), 0);

    env.hook_fixture("stop.json");
    assert_eq!(env.ingest_spawns(), 1);

    env.hook_fixture("session_end.json");
    assert_eq!(env.ingest_spawns(), 2);
}

#[test]
fn an_ingest_that_cannot_take_the_lock_exits_without_ingesting() {
    let env = TestEnv::new();
    env.hook_fixture("post_tool_use_bash.json");

    let holder = IngestLock::try_acquire(&env.paths)
        .unwrap()
        .expect("lock is free");
    assert_eq!(env.try_ingest(), IngestOutcome::AlreadyRunning);
    assert_eq!(env.tool_ranking(&Filter::default()), vec![]);

    drop(holder);
    env.ingest();
    assert_eq!(env.tool_ranking(&Filter::default())[0].stats.calls, 1);
}

#[test]
fn input_spooled_while_the_lock_holder_finishes_is_ingested_before_it_exits() {
    let env = TestEnv::new();
    // Hold the archive's write lock so the holder's pass stalls after it has
    // listed the (empty) spool: whatever arrives now is past its last pass.
    let blocker = env.db();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    std::thread::scope(|scope| {
        let holder = scope.spawn(|| env.try_ingest());
        std::thread::sleep(StdDuration::from_millis(300));

        env.hook_fixture("post_tool_use_bash.json");
        assert_eq!(env.try_ingest(), IngestOutcome::AlreadyRunning);

        blocker.execute_batch("COMMIT").unwrap();
        assert!(matches!(holder.join().unwrap(), IngestOutcome::Ran(_)));
    });

    let ranking = env.tool_ranking(&Filter::default());
    assert_eq!(ranking.len(), 1, "the late call was ingested: {ranking:?}");
    assert_eq!(ranking[0].stats.calls, 1);
}

/// Spools `count` Read calls spread over three sessions.
fn spool_reads(env: &TestEnv, count: usize) {
    for i in 0..count {
        env.hook_fixture_with("post_tool_use_read.json", |p| {
            p["session_id"] = json!(format!("session-{}", i % 3));
            p["tool_use_id"] = json!(format!("toolu_{i}"));
            p["duration_ms"] = json!(i);
        });
    }
}

#[test]
fn two_concurrent_ingest_processes_give_the_same_stats_as_one_run() {
    let single = TestEnv::new();
    spool_reads(&single, 300);
    single.ingest();

    let concurrent = TestEnv::new();
    spool_reads(&concurrent, 300);
    let outputs: Vec<_> = std::thread::scope(|scope| {
        let runs: Vec<_> = (0..2)
            .map(|_| scope.spawn(|| concurrent.run_bin(&["ingest"], b"")))
            .collect();
        runs.into_iter().map(|run| run.join().unwrap()).collect()
    });

    for out in &outputs {
        assert_eq!(out.status.code(), Some(0), "{out:?}");
    }
    let expected = single.tool_ranking(&Filter::default());
    assert_eq!(expected[0].stats.calls, 300);
    assert_eq!(concurrent.tool_ranking(&Filter::default()), expected);
}

#[test]
fn two_concurrent_ingest_threads_give_the_same_stats_as_one_run() {
    let single = TestEnv::new();
    spool_reads(&single, 300);
    single.ingest();

    let concurrent = TestEnv::new();
    spool_reads(&concurrent, 300);
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                barrier.wait();
                concurrent.try_ingest();
            });
        }
    });

    assert_eq!(
        concurrent.tool_ranking(&Filter::default()),
        single.tool_ranking(&Filter::default())
    );
}

const LIVE: &str = "live-session";
const ENDED: &str = "3f2b8c1e-7d4a-4e5b-9c6f-1a2b3c4d5e6f";

fn in_session(session: &'static str) -> impl FnOnce(&mut serde_json::Value) {
    move |p| p["session_id"] = json!(session)
}

#[test]
fn a_crashed_session_is_ingested_by_the_next_ingest_of_another_session() {
    let env = TestEnv::new();
    // The crashed session never sends Stop or SessionEnd, so it never
    // triggers an ingest of its own.
    env.hook_fixture_with("post_tool_use_read.json", in_session("crashed"));
    env.hook_fixture("post_tool_use_bash.json");
    env.hook_fixture("stop.json");

    env.ingest();

    let names: Vec<_> = env
        .tool_ranking(&Filter::default())
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["Bash", "Read"]);
}

#[test]
fn the_spool_of_an_ended_session_is_purged_and_a_live_one_kept() {
    let env = TestEnv::new();
    env.hook_fixture_with("post_tool_use_read.json", in_session(LIVE));
    env.hook_fixture_with("stop.json", in_session(LIVE));
    env.hook_fixture("post_tool_use_bash.json");
    env.hook_fixture("stop.json");
    env.hook_fixture("session_end.json");
    assert_eq!(env.spooled_sessions(), [ENDED, LIVE]);

    env.ingest();

    assert_eq!(env.spooled_sessions(), [LIVE]);
    assert_eq!(env.tool_ranking(&Filter::default()).len(), 2);
}

#[test]
fn an_idle_spool_is_purged_after_the_threshold() {
    let env = TestEnv::new();
    env.hook_fixture_with("post_tool_use_read.json", in_session(LIVE));

    env.advance(Duration::hours(23));
    env.ingest();
    assert_eq!(env.spooled_sessions(), [LIVE]);

    env.advance(Duration::hours(2));
    env.ingest();
    assert_eq!(env.spooled_sessions(), Vec::<String>::new());
    assert_eq!(env.tool_ranking(&Filter::default())[0].stats.calls, 1);
}

#[test]
fn a_spool_with_unread_input_is_not_purged() {
    let env = TestEnv::new();
    env.hook_fixture("post_tool_use_bash.json");
    env.hook_fixture("session_end.json");
    // A line still being written when ingest runs.
    let spool = env.paths.spool_dir().join(format!("{ENDED}.jsonl"));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&spool)
        .unwrap();
    std::io::Write::write_all(&mut file, b"{\"received_at\":").unwrap();

    env.ingest();

    assert_eq!(env.spooled_sessions(), [ENDED]);
}

#[test]
fn a_session_resumed_after_its_spool_was_purged_is_still_ingested_once() {
    let env = TestEnv::new();
    env.hook_fixture("post_tool_use_bash.json");
    env.hook_fixture("session_end.json");
    env.ingest();
    assert_eq!(env.spooled_sessions(), Vec::<String>::new());

    // `claude --resume` continues the same session id in a fresh spool.
    env.advance(Duration::minutes(5));
    env.hook_fixture_with("post_tool_use_read.json", |_| {});
    env.hook_fixture("stop.json");
    env.ingest();
    env.ingest();

    let calls: Vec<_> = env
        .tool_ranking(&Filter::default())
        .into_iter()
        .map(|t| (t.name, t.stats.calls))
        .collect();
    assert_eq!(calls, [("Bash".to_owned(), 1), ("Read".to_owned(), 1)]);
}

#[test]
fn hook_binary_returns_without_waiting_for_the_ingest_it_spawns() {
    let env = TestEnv::new();
    env.hook_fixture("post_tool_use_bash.json");
    // Hold the archive's write lock: the spawned ingest blocks on it, so a
    // hook that waited for its ingest could not return.
    let blocker = env.db();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

    let started = Instant::now();
    let out = env.run_bin(&["hook"], hook_fixture("stop.json").to_string().as_bytes());
    let elapsed = started.elapsed();

    assert_eq!(out.status.code(), Some(0));
    assert!(elapsed < StdDuration::from_secs(3), "hook took {elapsed:?}");
    assert_eq!(env.tool_ranking(&Filter::default()), vec![]);

    // Once the lock is released, the detached ingest completes on its own.
    blocker.execute_batch("COMMIT").unwrap();
    let deadline = Instant::now() + StdDuration::from_secs(20);
    while env.tool_ranking(&Filter::default()).is_empty() {
        assert!(
            Instant::now() < deadline,
            "detached ingest never ran; log: {}",
            env.log()
        );
        std::thread::sleep(StdDuration::from_millis(50));
    }
    assert_eq!(env.tool_ranking(&Filter::default())[0].name, "Bash");
}
