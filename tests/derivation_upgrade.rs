//! The one-time rebuild after an upgrade: when the derived tables were built
//! by an older claudit (an older or missing derivation version), the next
//! catch-up ingest rebuilds them once, as `claudit reingest` would, so
//! transcripts already read to their end yield what the new version
//! derives from them (e.g. tool calls from transcripts).
//!
//! Seam under test: seam 1 (`TestEnv::ingest`, the locked catch-up
//! `claudit ingest` and `claudit serve` run; typed stats and its
//! `IngestReport` out). Setting up "an archive an older claudit wrote"
//! cannot go through a public interface, since the older code is gone: the
//! setup removes the transcript tool calls and the derivation version from
//! the database directly, which is what such an archive looks like.
//! Assertions stay on stats reports and the ingest report.

mod common;

use claudit::stats::Filter;
use common::TestEnv;

/// Makes `env`'s archive look like one an older claudit wrote: no tool
/// calls derived from transcripts, no derivation version.
fn as_if_written_by_an_older_claudit(env: &TestEnv) {
    let conn = env.db();
    conn.execute("DELETE FROM tool_calls", []).unwrap();
    conn.execute(
        "DELETE FROM meta WHERE key = ?1",
        [claudit::ingest::META_DERIVATION_VERSION],
    )
    .unwrap();
}

fn calls(env: &TestEnv) -> u64 {
    env.tool_ranking(&Filter::default())
        .iter()
        .map(|t| t.stats.calls)
        .sum()
}

#[test]
fn an_archive_derived_by_an_older_claudit_is_rebuilt_once_on_the_next_ingest() {
    let env = TestEnv::new();
    env.drop_tool_calls_fixture();
    let first = env.ingest();
    assert!(!first.rebuilt, "a new archive has nothing to rebuild");
    assert_eq!(calls(&env), 5, "the fixture session's calls");

    as_if_written_by_an_older_claudit(&env);
    assert_eq!(calls(&env), 0);

    // The transcript was read to its end already: only a rebuild brings
    // its tool calls back.
    let upgrade = env.ingest();
    assert!(upgrade.rebuilt);
    assert_eq!(calls(&env), 5);
    assert_eq!(env.consumption(&Filter::default()).sessions, 1);

    // Once only: the version is now current.
    env.db().execute("DELETE FROM tool_calls", []).unwrap();
    let next = env.ingest();
    assert!(!next.rebuilt);
    assert_eq!(calls(&env), 0);
}
