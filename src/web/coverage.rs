//! What the time sections cover: time is measured only on sessions the
//! hooks recorded, never on sessions imported from transcripts.

use crate::stats::time::TimeCoverage;

use super::format;

/// The coverage note and empty state shared by every time section
/// (overview time row and activities, `/activities`, `/tools`).
pub(super) struct TimeNote {
    /// No hook-recorded session matches the filter: time sections show an
    /// empty state (`partials/time_empty.html`) instead of zeros.
    pub empty: bool,
    /// The day hook timing started (`2026-03-02`), if it has.
    pub since: Option<String>,
    /// `Time measured on N sessions recorded with hooks since D; X imported
    /// sessions not included.`, empty when nothing imported is left out.
    pub note: String,
}

impl TimeNote {
    pub fn new(coverage: &TimeCoverage) -> Self {
        let plural = |n: u64, word: &str| {
            format!(
                "{} {word}{}",
                format::count(n),
                if n == 1 { "" } else { "s" }
            )
        };
        let since = coverage.hooks_since.map(|at| {
            format::local_time(at)
                .split(' ')
                .next()
                .unwrap_or("")
                .to_owned()
        });
        let note = if coverage.imported_sessions > 0 && coverage.recorded_sessions > 0 {
            format!(
                "Time measured on {} recorded with hooks{}; {} imported from transcripts not included.",
                plural(coverage.recorded_sessions, "session"),
                since
                    .as_ref()
                    .map(|s| format!(" since {s}"))
                    .unwrap_or_default(),
                plural(coverage.imported_sessions, "session"),
            )
        } else {
            String::new()
        };
        Self {
            empty: coverage.recorded_sessions == 0,
            since,
            note,
        }
    }
}
