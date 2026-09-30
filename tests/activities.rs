//! Activities: what Claude's tool time is spent on (tests, builds, git, …).
//!
//! Seams under test:
//! - seam 1: hook fixtures go in through the hook entry point,
//!   `TestEnv::ingest` loads them, and assertions are made on the typed
//!   `claudit::stats::activities` reports, classified with a
//!   `claudit::activities::ActivityRules` passed in by the caller (the
//!   built-in rules, or rules parsed from a test TOML);
//! - `claudit::activities::ActivityRules::load`, over a `TestEnv`'s
//!   `CLAUDIT_HOME` (the user rules file and the error log).
//!
//! How single commands are classified is covered by the unit tests of
//! `src/activities.rs` (a pure function).
//!
//! Fixture session `9f4c2b7a-…` (`tests/fixtures/hooks/activities/`, all on
//! 2026-03-05 in acme-api), worked out by hand:
//!
//! | Activity | Calls | Failed | Durations (ms) | Total |
//! | --- | --- | --- | --- | --- |
//! | Tests | 4 | 2 | `cargo test` 12000, `cargo test -p api` 8000 (failed), `cargo nextest run` 5000, `pnpm test` 3000 (failed) | 28000 |
//! | Build & typecheck | 2 | 0 | `cargo build` 6000, `cargo check` 2000 | 8000 |
//! | Git & GitHub | 3 | 0 | `gh pr create` 1600, `git commit` 300, `git status` 100 | 2000 |
//! | Edit files | 3 | 0 | Edit 50, Edit 70, Write 30 | 150 |
//! | Other shell | 1 | 0 | `echo` 10 | 10 |
//!
//! Total tool time 38160 ms. Tests: nearest-rank median of
//! [3000, 5000, 8000, 12000] is rank 2 = 5000, p95 is rank 4 = 12000.

mod common;

use chrono::{NaiveDate, TimeZone, Utc};
use claudit::activities::ActivityRules;
use claudit::stats::Filter;
use claudit::stats::activities::{self, ActivityBreakdown};
use claudit::stats::tools::{CallStats, RankedCalls};
use common::TestEnv;

const SESSION: &str = "9f4c2b7a-5e6d-4f8a-b1c2-3d4e5f6a7b8c";

fn breakdown(env: &TestEnv, filter: &Filter, rules: &ActivityRules) -> ActivityBreakdown {
    activities::activity_breakdown(&env.db(), filter, rules).expect("activity_breakdown")
}

fn fixture_session() -> TestEnv {
    let env = TestEnv::new();
    env.replay_activities_session();
    env.ingest();
    env
}

fn names(b: &ActivityBreakdown) -> Vec<&str> {
    b.activities.iter().map(|a| a.activity.as_str()).collect()
}

fn ranked(name: &str, calls: u64, failures: u64, durations: &[u64]) -> RankedCalls {
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    RankedCalls {
        name: name.to_owned(),
        stats: CallStats {
            calls,
            failures,
            total_duration_ms: durations.iter().sum(),
            median_duration_ms: sorted.get(sorted.len().div_ceil(2) - 1).copied(),
            p95_duration_ms: sorted.last().copied(),
        },
    }
}

#[test]
fn tool_time_is_broken_down_by_activity() {
    let env = fixture_session();

    let b = breakdown(&env, &Filter::default(), ActivityRules::builtin());

    assert_eq!(
        names(&b),
        [
            "Tests",
            "Build & typecheck",
            "Git & GitHub",
            "Edit files",
            "Other shell"
        ]
    );
    assert_eq!(b.total_duration_ms, 38_160);

    let tests = &b.activities[0];
    assert_eq!(
        tests.stats,
        CallStats {
            calls: 4,
            failures: 2,
            total_duration_ms: 28_000,
            median_duration_ms: Some(5_000),
            p95_duration_ms: Some(12_000),
        }
    );
    assert!((b.share(tests) - 28_000.0 / 38_160.0).abs() < 1e-12);
    assert_eq!(tests.stats.failure_rate(), 0.5);
    assert_eq!(
        tests.top,
        vec![
            ranked("cargo test", 2, 1, &[12_000, 8_000]),
            ranked("cargo nextest", 1, 0, &[5_000]),
            ranked("pnpm test", 1, 1, &[3_000]),
        ]
    );

    let git = &b.activities[2];
    assert_eq!(git.stats.calls, 3);
    assert_eq!(git.stats.total_duration_ms, 2_000);
    let git_details: Vec<&str> = git.top.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(git_details, ["gh pr", "git commit", "git status"]);

    let edits = &b.activities[3];
    assert_eq!(
        edits.top,
        vec![
            ranked("Edit", 2, 0, &[50, 70]),
            ranked("Write", 1, 0, &[30])
        ]
    );
    let sum: f64 = b.activities.iter().map(|a| b.share(a)).sum();
    assert!((sum - 1.0).abs() < 1e-12);
}

#[test]
fn the_daily_series_has_each_activitys_time_per_day() {
    let env = fixture_session();
    env.replay_session_a_hooks(); // 2026-03-02: `cargo test`, Agent, Read.
    env.ingest();

    let b = breakdown(&env, &Filter::default(), ActivityRules::builtin());

    let day = |d: u32| NaiveDate::from_ymd_opt(2026, 3, d).unwrap();
    let series: Vec<(NaiveDate, &str, u64, u64)> = b
        .by_day
        .iter()
        .map(|d| (d.day, d.activity.as_str(), d.calls, d.duration_ms))
        .collect();
    assert_eq!(
        series,
        vec![
            (day(2), "Read files", 1, 700),
            (day(2), "Subagents", 1, 63_400),
            (day(2), "Tests", 1, 4_000),
            (day(5), "Build & typecheck", 2, 8_000),
            (day(5), "Edit files", 3, 150),
            (day(5), "Git & GitHub", 3, 2_000),
            (day(5), "Other shell", 1, 10),
            (day(5), "Tests", 4, 28_000),
        ]
    );
}

#[test]
fn a_session_has_its_own_breakdown() {
    let env = fixture_session();
    env.replay_session_a_hooks();
    env.ingest();

    let b = activities::session_activities(&env.db(), SESSION, ActivityRules::builtin())
        .expect("session_activities");

    assert_eq!(b.total_duration_ms, 38_160);
    assert_eq!(names(&b)[0], "Tests");
}

#[test]
fn changing_the_rules_reclassifies_without_reingesting() {
    let env = fixture_session();
    let before = breakdown(&env, &Filter::default(), ActivityRules::builtin());

    let user = ActivityRules::from_toml(
        r#"
        [[rule]]
        activity = "Rust"
        tools = ["Bash"]
        commands = ["cargo"]
        "#,
    )
    .unwrap();
    let after = breakdown(
        &env,
        &Filter::default(),
        &ActivityRules::builtin().with_overrides(&user),
    );

    assert_eq!(names(&before)[0], "Tests");
    // cargo test 12000 + 8000, cargo build 6000, cargo check 2000; the
    // `cd crates/api && cargo nextest run` call leads with cargo too.
    let rust = &after.activities[0];
    assert_eq!(rust.activity, "Rust");
    assert_eq!(rust.stats.calls, 5);
    assert_eq!(rust.stats.total_duration_ms, 33_000);
    let tests = after
        .activities
        .iter()
        .find(|a| a.activity == "Tests")
        .unwrap();
    assert_eq!(tests.stats.calls, 1, "only pnpm test is left");
    assert_eq!(after.total_duration_ms, before.total_duration_ms);
}

#[test]
fn filters_apply_to_the_calls() {
    let env = fixture_session();
    env.drop_projects_fixture();
    env.replay_session_a_hooks();
    env.ingest();

    let march_5 = Filter {
        from: Some(Utc.with_ymd_and_hms(2026, 3, 5, 0, 0, 0).unwrap()),
        ..Filter::default()
    };
    assert_eq!(
        breakdown(&env, &march_5, ActivityRules::builtin()).total_duration_ms,
        38_160
    );

    // Session A ran on Opus (its subagent on Haiku); the fixture session
    // has no transcript, hence no model.
    let opus = Filter {
        model: Some("claude-opus-5-5".to_owned()),
        ..Filter::default()
    };
    let b = breakdown(&env, &opus, ActivityRules::builtin());
    assert_eq!(names(&b), ["Subagents", "Tests"]);

    let elsewhere = Filter {
        project: Some("/Users/alice/code/web-app".to_owned()),
        ..Filter::default()
    };
    let b = breakdown(&env, &elsewhere, ActivityRules::builtin());
    assert!(b.activities.is_empty());
    assert!(b.by_day.is_empty());
    assert_eq!(b.total_duration_ms, 0);
}

#[test]
fn user_rules_from_claudit_home_come_first() {
    let env = fixture_session();
    std::fs::write(
        env.paths.activity_rules_file(),
        r#"
        version = "2026-10-01"

        [[rule]]
        activity = "Shipping"
        tools = ["Bash"]
        pattern = '^gh\s+pr\b'
        "#,
    )
    .unwrap();

    let loaded = ActivityRules::load(&env.paths);
    assert_eq!(loaded.problem, None);
    let b = breakdown(&env, &Filter::default(), &loaded.rules);

    let shipping = b.activities.iter().find(|a| a.activity == "Shipping");
    assert_eq!(shipping.map(|a| a.stats.total_duration_ms), Some(1_600));
    let git = b
        .activities
        .iter()
        .find(|a| a.activity == "Git & GitHub")
        .unwrap();
    assert_eq!(git.stats.calls, 2);
}

#[test]
fn an_invalid_user_rules_file_is_ignored_and_reported() {
    let env = fixture_session();
    std::fs::write(
        env.paths.activity_rules_file(),
        "[[rule]]\nactivity = \"Broken\"\ntools = [\"Bash\"]\npattern = \"(unclosed\"\n",
    )
    .unwrap();

    let loaded = ActivityRules::load(&env.paths);

    let problem = loaded.problem.expect("the problem is reported");
    assert!(problem.contains("activities.toml"), "{problem}");
    assert!(
        problem.contains("rule 1 (Broken): invalid pattern"),
        "{problem}"
    );
    assert!(env.log().contains("rule 1 (Broken)"), "{}", env.log());
    let b = breakdown(&env, &Filter::default(), &loaded.rules);
    assert_eq!(names(&b)[0], "Tests", "the built-in rules still apply");
}
