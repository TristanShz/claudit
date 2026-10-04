//! Session export: a Markdown summary of a session (its general
//! statistics) or a full export (plus every turn's prompt and trace), for
//! pasting into a conversation with an AI.
//!
//! Seams under test: seam 1 (`claudit::export::{session_markdown,
//! resolve_session}` over an ingested archive) and the `claudit export`
//! command, run as the real binary.
//!
//! The scenario is the synthetic session `c4e8a2f0-…`
//! (`TestEnv::populate_trace_session`, `tests/fixtures/hooks/README.md`).

mod common;

use claudit::activities::ActivityRules;
use claudit::export::{self, ExportLevel};
use claudit::pricing::PriceTable;
use common::TestEnv;

const SESSION: &str = "c4e8a2f0-5b3d-4e7a-9f1c-2d6b8e0a4c7e";

fn trace_session() -> TestEnv {
    let env = TestEnv::new();
    env.populate_trace_session();
    env.ingest();
    env
}

fn markdown(env: &TestEnv, level: ExportLevel) -> String {
    export::session_markdown(
        &env.db(),
        SESSION,
        level,
        ActivityRules::builtin(),
        PriceTable::builtin(),
    )
    .expect("session_markdown")
    .expect("the session is known")
}

#[test]
fn the_summary_gives_the_sessions_general_statistics() {
    let env = trace_session();

    let md = markdown(&env, ExportLevel::Summary);

    assert!(
        md.starts_with(&format!("# Claude Code session {SESSION}\n")),
        "{md}"
    );
    for section in [
        "## Session",
        "## Where the time goes",
        "## Activities",
        "## Tools",
        "## Subagents",
        "## Turns",
    ] {
        assert!(
            md.contains(&format!("\n{section}\n")),
            "lacks {section}:\n{md}"
        );
    }
    assert!(md.contains("fix/flaky-login"), "{md}");
    // Turn 1 runs 18 s and its background subagent goes on alone 18 → 40 s.
    assert!(md.contains("| Background subagents | 22.0 s |"), "{md}");
    assert!(md.contains("Investigate the flaky test"), "{md}");
    // Four turns, timed or not, one row each.
    for n in 1..=4 {
        assert!(md.contains(&format!("\n| {n} | ")), "lacks turn {n}:\n{md}");
    }
    // No per-turn detail.
    assert!(!md.contains("### Turn"), "{md}");
    assert!(!md.contains("cargo test login -- --nocapture"), "{md}");
}

#[test]
fn the_full_export_adds_each_turns_prompt_and_trace() {
    let env = trace_session();

    let summary = markdown(&env, ExportLevel::Summary);
    let md = markdown(&env, ExportLevel::Full);

    // Everything the summary has, then the turns in detail.
    let (head, detail) = md.split_once("\n## Turn details\n").expect("details");
    assert_eq!(
        head.trim_end(),
        summary.trim_end().replace("Summary export", "Full export")
    );
    assert!(detail.contains("### Turn 2"), "{detail}");
    assert!(detail.contains("Fix the flaky login test"), "{detail}");
    // The subagent's failed call, its thread and its error.
    assert!(
        detail.contains("cargo test login -- --nocapture"),
        "{detail}"
    );
    assert!(detail.contains("Exit code 101"), "{detail}");
    assert!(detail.contains("Explore"), "{detail}");
}

#[test]
fn an_unknown_session_exports_nothing() {
    let env = trace_session();

    let md = export::session_markdown(
        &env.db(),
        "00000000-0000-0000-0000-000000000000",
        ExportLevel::Summary,
        ActivityRules::builtin(),
        PriceTable::builtin(),
    )
    .unwrap();

    assert_eq!(md, None);
}

#[test]
fn a_session_is_found_by_a_unique_prefix_of_its_id() {
    let env = trace_session();
    let conn = env.db();

    assert_eq!(export::resolve_session(&conn, "c4e8a2f0").unwrap(), SESSION);
    assert_eq!(export::resolve_session(&conn, SESSION).unwrap(), SESSION);
    assert!(export::resolve_session(&conn, "ffffffff").is_err());
    assert!(export::resolve_session(&conn, "").is_err());
}

#[test]
fn claudit_export_prints_the_export_or_writes_it_to_a_file() {
    let env = trace_session();

    let out = env.run_bin(&["export", "c4e8a2f0"], b"");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.starts_with(&format!("# Claude Code session {SESSION}\n")));
    assert!(!stdout.contains("## Turn details"));

    let file = env.paths.home().join("export.md");
    let out = env.run_bin(
        &["export", "c4e8a2f0", "--full", "-o", file.to_str().unwrap()],
        b"",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("## Turn details"));

    let out = env.run_bin(&["export", "ffffffff"], b"");
    assert!(!out.status.success());
}
