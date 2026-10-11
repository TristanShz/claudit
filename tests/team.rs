//! Team comparison: the period reports in a folder, member by member
//! against the team median.
//!
//! Seam under test: `claudit::team::{load_dir, periods, compare,
//! TeamSettings}` over reports `claudit::report::build` wrote from the
//! synthetic session `c4e8a2f0-…` (`TestEnv::populate_trace_session`); the
//! other members are that report with another author and edited figures.

mod common;

use chrono::{Duration, NaiveDate};
use claudit::activities::ActivityRules;
use claudit::pricing::PriceTable;
use claudit::report::{self, Period, Report, ReportOptions};
use claudit::team::{self, Deviation, Row, TeamSettings};
use common::{TestEnv, t0};

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn march() -> Period {
    Period {
        from: day("2026-03-01"),
        to: day("2026-03-31"),
    }
}

/// Alice's report of March, from the trace session.
fn alice() -> Report {
    let env = TestEnv::new();
    env.populate_trace_session();
    env.ingest();
    report::build(
        &env.db(),
        &ReportOptions {
            author: "Alice".to_owned(),
            period: march(),
            project: None,
            branch: None,
            model: None,
            include_prompts: false,
        },
        ActivityRules::builtin(),
        PriceTable::builtin(),
        t0(),
        &|cwd| cwd.rsplit('/').next().unwrap().to_owned(),
    )
    .unwrap()
}

/// Alice's report under another author's name.
fn member(base: &Report, author: &str) -> Report {
    Report {
        author: author.to_owned(),
        ..base.clone()
    }
}

fn row<'a>(rows: &'a [Row], label: &str) -> &'a Row {
    rows.iter()
        .find(|r| r.label == label)
        .unwrap_or_else(|| panic!("no row {label}"))
}

fn write(dir: &std::path::Path, name: &str, report: &Report) {
    std::fs::write(dir.join(name), serde_json::to_string(report).unwrap()).unwrap();
}

#[test]
fn a_folder_yields_its_reports_and_lists_the_files_it_skipped() {
    let alice = alice();
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "alice.json", &alice);
    write(dir.path(), "bob.JSON", &member(&alice, "Bob"));
    std::fs::write(dir.path().join("notes.json"), r#"{"todo": []}"#).unwrap();
    std::fs::write(dir.path().join("README.txt"), "not json").unwrap();
    std::fs::create_dir(dir.path().join("archive")).unwrap();
    write(&dir.path().join("archive"), "old.json", &alice);

    let loaded = team::load_dir(dir.path()).unwrap();

    let mut authors: Vec<&str> = loaded.reports.iter().map(|r| r.author.as_str()).collect();
    authors.sort();
    assert_eq!(authors, ["Alice", "Bob"]);
    assert_eq!(loaded.skipped.len(), 1, "{:?}", loaded.skipped);
    assert_eq!(loaded.skipped[0].file, "notes.json");
    assert!(loaded.skipped[0].reason.contains("not a claudit report"));

    assert!(team::load_dir(&dir.path().join("missing")).is_err());
}

#[test]
fn the_period_most_members_reported_comes_first() {
    let alice = alice();
    let september = Period {
        from: day("2026-09-01"),
        to: day("2026-09-30"),
    };
    let reports = vec![
        alice.clone(),
        member(&alice, "Bob"),
        // Alice twice in March counts once.
        Report {
            generated_at: alice.generated_at + Duration::hours(1),
            ..alice.clone()
        },
        Report {
            period: september,
            ..member(&alice, "Carol")
        },
    ];

    let choices = team::periods(&reports);

    assert_eq!(choices.len(), 2);
    assert_eq!((choices[0].period, choices[0].members), (march(), 2));
    assert_eq!((choices[1].period, choices[1].members), (september, 1));
}

#[test]
fn members_are_compared_on_rates_against_the_team_median() {
    let alice = alice();
    let mut bob = member(&alice, "Bob");
    // Bob calls tools three times as often per turn, and waits nowhere.
    bob.totals.tool_calls *= 3;
    bob.time.waiting = 0;
    // An older report of Bob's is superseded by the one above.
    let stale = Report {
        generated_at: alice.generated_at - Duration::days(1),
        ..member(&alice, "Bob")
    };
    let mut carol = member(&alice, "Carol");
    carol.skills.clear();

    let c = team::compare(&[stale, carol, bob, alice.clone()], march());

    assert_eq!(c.members, ["Alice", "Bob", "Carol"]);
    let calls = row(&c.metrics, "Tool calls per turn");
    let base = alice.totals.tool_calls as f64 / alice.totals.turns as f64;
    assert_eq!(calls.median, Some(base));
    assert_eq!(calls.values[1], Some(base * 3.0));
    assert_eq!(
        calls.deviations,
        [Deviation::Typical, Deviation::Above, Deviation::Typical]
    );
    let waiting = row(&c.metrics, "Waiting on you");
    assert_eq!(waiting.values[1], Some(0.0));
    assert_eq!(waiting.deviations[1], Deviation::Below);
    // Totals are listed too, unflagged when equal.
    assert_eq!(
        row(&c.metrics, "Sessions").values,
        [Some(1.0), Some(1.0), Some(1.0)]
    );

    // Items any member has, at zero for those who lack them.
    if let Some(skill) = alice.skills.first() {
        let skill_row = row(&c.skills, &skill.skill);
        assert_eq!(skill_row.values[2], Some(0.0));
        assert_eq!(skill_row.users(), 2);
    }
    let bash = row(&c.tools, "Bash");
    assert_eq!(bash.users(), 3);
}

#[test]
fn a_member_without_timed_turns_has_no_time_figures() {
    let alice = alice();
    let mut imported = member(&alice, "Dan");
    imported.totals.timed_turns = 0;
    imported.time = Default::default();

    let c = team::compare(&[alice, imported], march());

    let model = row(&c.metrics, "Model");
    assert!(model.values[0].is_some());
    assert_eq!(model.values[1], None);
    assert_eq!(row(&c.metrics, "Time per turn").values[1], None);
}

#[test]
fn the_team_folder_is_remembered() {
    let env = TestEnv::new();
    assert_eq!(TeamSettings::load(&env.paths), TeamSettings::default());

    let settings = TeamSettings {
        dir: Some("/shared/team-reports".into()),
    };
    settings.save(&env.paths).unwrap();

    assert_eq!(TeamSettings::load(&env.paths), settings);
}
