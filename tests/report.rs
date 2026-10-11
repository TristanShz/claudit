//! Period report: one person's usage over a period as versioned JSON, to
//! share with a team and compare.
//!
//! Seams under test: seam 1 (`claudit::report::{build, read}` over an
//! ingested archive) and the `claudit report` command, run as the real
//! binary.
//!
//! The scenario is the synthetic session `c4e8a2f0-…` of 2026-03-07 in
//! `/Users/alice/code/acme-api` (`TestEnv::populate_trace_session`).

mod common;

use chrono::NaiveDate;
use claudit::activities::ActivityRules;
use claudit::pricing::PriceTable;
use claudit::report::{self, Period, Report, ReportOptions};
use common::{TestEnv, t0};

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn trace_session() -> TestEnv {
    let env = TestEnv::new();
    env.populate_trace_session();
    env.ingest();
    env
}

fn options(from: &str, to: &str) -> ReportOptions {
    ReportOptions {
        author: "Alice Martin".to_owned(),
        period: Period {
            from: day(from),
            to: day(to),
        },
        project: None,
        branch: None,
        model: None,
        include_prompts: false,
    }
}

/// Names a project by its directory, as `git_project_name` does outside a
/// repository.
fn by_dir(cwd: &str) -> String {
    cwd.rsplit('/').next().unwrap().to_owned()
}

fn build(env: &TestEnv, options: &ReportOptions) -> Report {
    report::build(
        &env.db(),
        options,
        ActivityRules::builtin(),
        PriceTable::builtin(),
        t0(),
        &by_dir,
    )
    .expect("build")
}

#[test]
fn a_report_aggregates_the_period_without_prompts_nor_paths() {
    let env = trace_session();

    let r = build(&env, &options("2026-03-01", "2026-03-31"));

    assert_eq!(r.format, report::FORMAT);
    assert_eq!(r.schema_version, report::SCHEMA_VERSION);
    assert_eq!(r.author, "Alice Martin");
    assert_eq!(r.totals.sessions, 1);
    assert_eq!(r.totals.turns, 4);
    assert!(r.totals.timed_turns > 0, "{r:#?}");
    assert!(r.totals.tool_calls > 0, "{r:#?}");
    assert!(r.totals.tokens.output > 0, "{r:#?}");
    assert!(r.time.model > 0 && r.time.background > 0, "{:?}", r.time);
    assert!(r.tools.iter().any(|t| t.name == "Bash"), "{:?}", r.tools);
    assert!(
        r.subagents.iter().any(|s| s.agent_type == "Explore"),
        "{:?}",
        r.subagents
    );
    assert_eq!(r.projects.len(), 1);
    assert_eq!(r.projects[0].name, "acme-api");
    assert_eq!(r.projects[0].sessions, 1);
    assert_eq!(r.by_day.len(), 1);
    assert_eq!(r.by_day[0].day, day("2026-03-07"));
    assert_eq!(r.by_day[0].sessions, 1);
    assert_eq!(r.prompts, None);

    let json = serde_json::to_string(&r).unwrap();
    assert!(!json.contains("/Users/alice"), "{json}");
    assert!(!json.contains("Fix the flaky login test"), "{json}");
    assert!(!json.contains("\"prompts\""), "{json}");
}

#[test]
fn prompts_are_added_only_on_request() {
    let env = trace_session();
    let mut with_prompts = options("2026-03-07", "2026-03-07");
    with_prompts.include_prompts = true;

    let r = build(&env, &with_prompts);

    // Two typed prompts, a command included; the two turns Claude Code
    // opened (task notifications) are left out.
    let prompts = r.prompts.expect("prompts");
    let texts: Vec<&str> = prompts.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, ["/implement 12", "Fix the flaky login test"]);
    assert!(prompts.iter().all(|p| p.project == "acme-api"));
}

#[test]
fn a_period_without_activity_reports_nothing() {
    let env = trace_session();

    let r = build(&env, &options("2026-03-08", "2026-03-31"));

    assert_eq!(r.totals.sessions, 0);
    assert_eq!(r.totals.turns, 0);
    assert_eq!(r.totals.tool_calls, 0);
    assert!(r.projects.is_empty() && r.by_day.is_empty() && r.tools.is_empty());
}

#[test]
fn a_project_filter_is_named_like_the_projects() {
    let env = trace_session();
    let mut filtered = options("2026-03-01", "2026-03-31");
    filtered.project = Some("/Users/alice/code/acme-api".to_owned());

    let r = build(&env, &filtered);

    assert_eq!(r.totals.sessions, 1);
    assert_eq!(r.filter.project.as_deref(), Some("acme-api"));
}

#[test]
fn a_report_reads_back_and_other_json_is_refused() {
    let env = trace_session();
    let mut with_prompts = options("2026-03-01", "2026-03-31");
    with_prompts.include_prompts = true;
    let r = build(&env, &with_prompts);

    let json = serde_json::to_string_pretty(&r).unwrap();
    assert_eq!(report::read(&json).unwrap(), r);

    assert!(report::read(r#"{"hello": 1}"#).is_err());
    let newer = json.replacen(
        &format!("\"schema_version\": {}", report::SCHEMA_VERSION),
        &format!("\"schema_version\": {}", report::SCHEMA_VERSION + 1),
        1,
    );
    let err = report::read(&newer).unwrap_err().to_string();
    assert!(err.contains("newer"), "{err}");
}

#[test]
fn claudit_report_writes_into_a_directory_under_a_default_name() {
    let env = trace_session();
    let dir = env.paths.home().join("team");
    std::fs::create_dir_all(&dir).unwrap();

    let out = env.run_bin(
        &[
            "report",
            "--from",
            "2026-03-07",
            "--to",
            "2026-03-07",
            "--author",
            "Alice Martin",
            "-o",
            dir.to_str().unwrap(),
        ],
        b"",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let file = dir.join("claudit-report-alice-martin-2026-03-07_2026-03-07.json");
    let r = report::read(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(r.totals.sessions, 1);
    assert_eq!(r.author, "Alice Martin");

    let out = env.run_bin(
        &[
            "report",
            "--from",
            "2026-03-07",
            "--days",
            "3",
            "--author",
            "A",
        ],
        b"",
    );
    assert!(!out.status.success());
}

#[test]
fn a_removed_worktree_is_named_by_its_repository() {
    use std::process::Command;
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("api");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["remote", "add", "origin", "git@github.com:acme/api.git"]);

    let gone = repo.join(".claude/worktrees/fix-login");
    assert_eq!(
        report::git_project_name(gone.to_str().unwrap()),
        "github.com/acme/api"
    );
    let outside = root.path().join("scratch");
    std::fs::create_dir_all(&outside).unwrap();
    assert_eq!(
        report::git_project_name(outside.to_str().unwrap()),
        "scratch"
    );
}
