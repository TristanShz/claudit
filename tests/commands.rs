//! Bash commands: which commands run most, and which take the most time,
//! by command key (`pnpm exec vitest`, `git status`), overall and per
//! session.
//!
//! Seam under test: seam 1 only. Hook fixtures go in through the hook
//! entry point, transcripts are dropped in place, `TestEnv::ingest` loads
//! them, and assertions are made on the typed
//! `claudit::stats::commands` reports (classified with the built-in
//! activity rules). How one command line becomes a command key is covered
//! by the unit tests of `src/activities.rs` (a pure function).
//!
//! The archive of [`archive`], worked out by hand:
//!
//! - session `9f4c2b7a-…` (`tests/fixtures/hooks/activities/`, acme-api,
//!   2026-03-05, hooks only; see `tests/activities.rs`);
//! - session `7a1e3c5b-…` (`tests/fixtures/hooks/commands/`, web-app,
//!   2026-03-06, hooks only): `pnpm exec vitest` 42 s, 61 s and 38 s
//!   (failed), `git status` 90 and 110 ms, `git diff` 150 ms,
//!   `pnpm run build` 9 s;
//! - session `6e2d9b47-…` (`tests/fixtures/transcripts/tool_calls/`,
//!   toolbox, imported from its transcript): `git status --short` and a
//!   failed `cargo test --all`, counted but never timed.
//!
//! | Command | Activity | Runs | Timed | Failed | Durations (ms) |
//! | --- | --- | --- | --- | --- | --- |
//! | `git status` | Git & GitHub | 4 | 3 | 0 | 100, 90, 110 |
//! | `pnpm exec vitest` | Tests | 3 | 3 | 1 | 42000, 61000, 38000 |
//! | `cargo test` | Tests | 3 | 2 | 2 | 12000, 8000 |
//! | `pnpm run build` | Build & typecheck | 1 | 1 | 0 | 9000 |
//! | `cargo build` | Build & typecheck | 1 | 1 | 0 | 6000 |
//! | `cargo nextest` | Tests | 1 | 1 | 0 | 5000 |
//! | `pnpm test` | Tests | 1 | 1 | 1 | 3000 |
//! | `cargo check` | Build & typecheck | 1 | 1 | 0 | 2000 |
//! | `gh pr` | Git & GitHub | 1 | 1 | 0 | 1600 |
//! | `git commit` | Git & GitHub | 1 | 1 | 0 | 300 |
//! | `git diff` | Git & GitHub | 1 | 1 | 0 | 150 |
//! | `echo` | Other shell | 1 | 1 | 0 | 10 |
//!
//! 19 runs, 17 timed, 188 360 ms of Bash time. Percentiles are
//! nearest-rank over the timed runs (see `stats::tools`).

mod common;

use chrono::{TimeZone, Utc};
use claudit::activities::ActivityRules;
use claudit::stats::Filter;
use claudit::stats::commands::{self, CommandRanking, CommandSort, CommandStat};
use claudit::stats::tools::CallStats;
use common::TestEnv;

const COMMANDS_SESSION: &str = "7a1e3c5b-2d4f-4a6b-8c9d-0e1f2a3b4c5d";
const BASH_TIME_MS: u64 = 188_360;

fn archive() -> TestEnv {
    let env = TestEnv::new();
    env.replay_activities_session();
    env.replay_commands_session();
    env.drop_tool_calls_fixture();
    env.ingest();
    env
}

fn ranking(env: &TestEnv, filter: &Filter, sort: CommandSort) -> CommandRanking {
    commands::command_ranking(&env.db(), filter, ActivityRules::builtin(), sort)
        .expect("command_ranking")
}

fn keys(ranking: &CommandRanking) -> Vec<&str> {
    ranking
        .commands
        .iter()
        .map(|c| c.command.as_str())
        .collect()
}

/// `(runs, failures, timed durations in any order)`.
fn stats(calls: u64, failures: u64, durations: &[u64]) -> CallStats {
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    let rank = |p: usize| (sorted.len() * p).div_ceil(100).max(1) - 1;
    CallStats {
        calls,
        failures,
        total_duration_ms: durations.iter().sum(),
        median_duration_ms: sorted.get(rank(50)).copied(),
        p95_duration_ms: sorted.get(rank(95)).copied(),
        timed_calls: durations.len() as u64,
    }
}

#[test]
fn commands_are_ranked_by_runs_with_durations_from_hook_timed_runs_only() {
    let env = archive();

    let r = env.command_ranking(&Filter::default());

    assert_eq!(
        keys(&r),
        [
            "git status",
            "pnpm exec vitest",
            "cargo test",
            "pnpm run build",
            "cargo build",
            "cargo nextest",
            "pnpm test",
            "cargo check",
            "gh pr",
            "git commit",
            "git diff",
            "echo",
        ]
    );
    assert_eq!(
        (r.calls, r.timed_calls, r.total_duration_ms),
        (19, 17, BASH_TIME_MS)
    );

    // One run of `git status` is known only from a transcript: counted,
    // never timed.
    assert_eq!(
        r.commands[0],
        CommandStat {
            command: "git status".to_owned(),
            activity: "Git & GitHub".to_owned(),
            stats: CallStats {
                calls: 4,
                failures: 0,
                total_duration_ms: 300,
                median_duration_ms: Some(100),
                p95_duration_ms: Some(110),
                timed_calls: 3,
            },
            share_of_bash_time: 300.0 / BASH_TIME_MS as f64,
        }
    );
    let vitest = &r.commands[1];
    assert_eq!(vitest.activity, "Tests");
    assert_eq!(vitest.stats, stats(3, 1, &[42_000, 61_000, 38_000]));
    assert_eq!(
        (
            vitest.stats.median_duration_ms,
            vitest.stats.p95_duration_ms
        ),
        (Some(42_000), Some(61_000))
    );
    assert!((vitest.share_of_bash_time - 141_000.0 / 188_360.0).abs() < 1e-12);
    // The failed `cargo test --all` of the imported session counts as a
    // run and a failure.
    assert_eq!(r.commands[2].stats, stats(3, 2, &[12_000, 8_000]));

    let shares: f64 = r.commands.iter().map(|c| c.share_of_bash_time).sum();
    assert!((shares - 1.0).abs() < 1e-12);
}

#[test]
fn the_ranking_sorts_by_total_time_median_p95_or_failures() {
    let env = archive();
    let sorted = |sort| {
        ranking(&env, &Filter::default(), sort)
            .commands
            .into_iter()
            .map(|c| c.command)
            .collect::<Vec<_>>()
    };

    // `git status` and `git commit` both took 300 ms: more runs first.
    assert_eq!(
        sorted(CommandSort::Total),
        [
            "pnpm exec vitest",
            "cargo test",
            "pnpm run build",
            "cargo build",
            "cargo nextest",
            "pnpm test",
            "cargo check",
            "gh pr",
            "git status",
            "git commit",
            "git diff",
            "echo",
        ]
    );
    assert_eq!(
        sorted(CommandSort::Median)[..4],
        [
            "pnpm exec vitest",
            "pnpm run build",
            "cargo test",
            "cargo build"
        ]
    );
    assert_eq!(
        sorted(CommandSort::Median)[9..],
        ["git diff", "git status", "echo"]
    );
    assert_eq!(
        sorted(CommandSort::P95)[..4],
        [
            "pnpm exec vitest",
            "cargo test",
            "pnpm run build",
            "cargo build"
        ]
    );
    assert_eq!(
        sorted(CommandSort::Failures)[..4],
        ["cargo test", "pnpm exec vitest", "pnpm test", "git status"]
    );
}

#[test]
fn commands_never_timed_by_the_hooks_come_last_in_the_duration_orders() {
    let env = archive();
    let toolbox = Filter {
        project: Some("/Users/alice/code/toolbox".to_owned()),
        ..Filter::default()
    };
    let web_app = Filter {
        project: Some("/Users/alice/code/web-app".to_owned()),
        ..Filter::default()
    };

    // Imported only: counted, no durations.
    let imported = ranking(&env, &toolbox, CommandSort::Median);
    assert_eq!(keys(&imported), ["cargo test", "git status"]);
    assert_eq!(imported.commands[0].stats, stats(1, 1, &[]));
    assert_eq!(imported.commands[0].stats.median_duration_ms, None);
    assert_eq!(
        (
            imported.calls,
            imported.timed_calls,
            imported.total_duration_ms
        ),
        (2, 0, 0)
    );
    assert_eq!(imported.commands[0].share_of_bash_time, 0.0);

    // Mixed with timed commands, they sort below the fastest one.
    let mut mixed = ranking(&env, &web_app, CommandSort::Calls);
    mixed.commands.extend(imported.commands);
    for sort in [CommandSort::Median, CommandSort::P95] {
        mixed.sort(sort);
        assert_eq!(
            keys(&mixed)[2..],
            ["git diff", "git status", "cargo test", "git status"],
            "{sort:?}"
        );
    }
}

#[test]
fn a_project_filter_isolates_that_projects_vitest_time() {
    let env = archive();
    let web_app = Filter {
        project: Some("/Users/alice/code/web-app".to_owned()),
        ..Filter::default()
    };

    let r = ranking(&env, &web_app, CommandSort::Total);

    assert_eq!(
        keys(&r),
        [
            "pnpm exec vitest",
            "pnpm run build",
            "git status",
            "git diff"
        ]
    );
    assert_eq!(r.total_duration_ms, 150_350);
    let vitest = &r.commands[0];
    assert_eq!(vitest.stats.total_duration_ms, 141_000);
    assert!((vitest.share_of_bash_time - 141_000.0 / 150_350.0).abs() < 1e-12);
    assert_eq!(r.commands[2].stats, stats(2, 0, &[90, 110]));

    // The date range and the other dimensions apply too.
    let march_6 = Filter {
        from: Some(Utc.with_ymd_and_hms(2026, 3, 6, 0, 0, 0).unwrap()),
        to: Some(Utc.with_ymd_and_hms(2026, 3, 7, 0, 0, 0).unwrap()),
        ..Filter::default()
    };
    assert_eq!(ranking(&env, &march_6, CommandSort::Total), r);
    let acme = Filter {
        project: Some("/Users/alice/code/acme-api".to_owned()),
        ..Filter::default()
    };
    assert!(
        !keys(&ranking(&env, &acme, CommandSort::Calls)).contains(&"pnpm exec vitest"),
        "vitest ran only in web-app"
    );
    let feature_branch = Filter {
        branch: Some("feat/login".to_owned()),
        ..Filter::default()
    };
    assert_eq!(
        ranking(&env, &feature_branch, CommandSort::Calls),
        CommandRanking::default(),
        "hook-only sessions have no branch"
    );
}

#[test]
fn a_session_has_its_own_commands() {
    let env = archive();
    let web_app = Filter {
        project: Some("/Users/alice/code/web-app".to_owned()),
        ..Filter::default()
    };

    let session = commands::session_commands(
        &env.db(),
        COMMANDS_SESSION,
        ActivityRules::builtin(),
        CommandSort::Total,
    )
    .expect("session_commands");

    assert_eq!(session, ranking(&env, &web_app, CommandSort::Total));
    assert_eq!(session.calls, 7);
    let unknown = commands::session_commands(
        &env.db(),
        "00000000-0000-4000-8000-000000000000",
        ActivityRules::builtin(),
        CommandSort::Calls,
    )
    .expect("session_commands");
    assert_eq!(unknown, CommandRanking::default());
}

#[test]
fn user_rules_change_the_activity_of_a_command_without_reingesting() {
    let env = archive();
    let user = ActivityRules::from_toml(
        r#"
        [[rule]]
        activity = "Unit tests"
        tools = ["Bash"]
        pattern = '\bvitest\b'
        "#,
    )
    .unwrap();

    let r = commands::command_ranking(
        &env.db(),
        &Filter::default(),
        &ActivityRules::builtin().with_overrides(&user),
        CommandSort::Total,
    )
    .unwrap();

    assert_eq!(r.commands[0].command, "pnpm exec vitest");
    assert_eq!(r.commands[0].activity, "Unit tests");
}

#[test]
fn ingesting_again_changes_nothing() {
    let env = archive();
    let before = env.command_ranking(&Filter::default());

    env.replay_commands_session();
    env.ingest();

    assert_eq!(env.command_ranking(&Filter::default()), before);
}
