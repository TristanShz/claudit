//! The Bash commands table (`/commands`, the overview block, `/tools`, the
//! session page): rows, sortable column heads and the coverage note.

use serde::Deserialize;

use super::format;
use super::rows::activity_color;
use crate::stats::commands::{CommandRanking, CommandSort};

/// `?sort=` of the pages with a sortable commands table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct SortParam {
    pub sort: String,
}

/// The sorts a table offers, with their query value and column label.
const SORTS: [(CommandSort, &str, &str); 5] = [
    (CommandSort::Calls, "runs", "Runs"),
    (CommandSort::Total, "total", "Total time"),
    (CommandSort::Median, "median", "Median"),
    (CommandSort::P95, "p95", "p95"),
    (CommandSort::Failures, "failures", "Failures"),
];

impl SortParam {
    /// The requested sort; `default` when absent or unknown.
    pub fn sort_or(&self, default: CommandSort) -> CommandSort {
        SORTS
            .iter()
            .find(|(_, name, _)| *name == self.sort)
            .map_or(default, |(sort, _, _)| *sort)
    }
}

/// A command of the table, formatted.
pub(super) struct CommandRow {
    pub command: String,
    pub activity: String,
    /// Palette slot of its activity.
    pub color: usize,
    pub runs: u64,
    /// Runs the hooks timed (the durations are measured on these).
    pub timed_runs: u64,
    pub total: String,
    /// Share of the table's Bash time, e.g. `42%` (`–` without time).
    pub share: String,
    pub median: String,
    pub p95: String,
    pub failures: u64,
}

/// A numeric column head: its label, and the link that sorts by it (empty
/// for a table that is not sortable).
pub(super) struct SortHead {
    pub label: &'static str,
    pub href: String,
    /// The table is sorted by this column.
    pub active: bool,
}

/// Everything a commands table renders.
pub(super) struct CommandsTable {
    pub rows: Vec<CommandRow>,
    pub heads: Vec<SortHead>,
    /// `Time measured on N of M runs recorded with hooks.`, empty when every
    /// run was timed.
    pub coverage: String,
}

impl CommandsTable {
    /// The first `limit` commands of `ranking`, as sorted. `link` gives each
    /// sort's link (from its query value), or `None` for fixed heads.
    pub fn new(
        ranking: &CommandRanking,
        limit: usize,
        sorted_by: Option<CommandSort>,
        link: Option<&dyn Fn(&str) -> String>,
    ) -> Self {
        let optional = |ms: Option<u64>| ms.map_or_else(|| "–".to_owned(), format::duration_ms);
        Self {
            rows: ranking
                .commands
                .iter()
                .take(limit)
                .map(|c| CommandRow {
                    command: c.command.clone(),
                    color: activity_color(&c.activity),
                    activity: c.activity.clone(),
                    runs: c.stats.calls,
                    timed_runs: c.stats.timed_calls,
                    total: if c.stats.timed_calls == 0 {
                        "–".to_owned()
                    } else {
                        format::duration_ms(c.stats.total_duration_ms)
                    },
                    share: if c.stats.timed_calls == 0 || ranking.total_duration_ms == 0 {
                        "–".to_owned()
                    } else {
                        format!("{:.0}%", c.share_of_bash_time * 100.0)
                    },
                    median: optional(c.stats.median_duration_ms),
                    p95: optional(c.stats.p95_duration_ms),
                    failures: c.stats.failures,
                })
                .collect(),
            heads: SORTS
                .iter()
                .map(|(sort, name, label)| SortHead {
                    label,
                    href: link.map(|link| link(name)).unwrap_or_default(),
                    active: sorted_by == Some(*sort),
                })
                .collect(),
            coverage: coverage(ranking),
        }
    }
}

/// The coverage note: how many runs the durations are measured on.
pub(super) fn coverage(ranking: &CommandRanking) -> String {
    if ranking.timed_calls == ranking.calls {
        return String::new();
    }
    format!(
        "Time measured on {} of {} runs recorded with hooks; runs imported from transcripts are counted without time.",
        format::count(ranking.timed_calls),
        format::count(ranking.calls),
    )
}

/// `path` with the filter `query` (`""` or `?…`) and `sort=<name>`, then
/// `anchor` (`""` or `#…`).
pub(super) fn sort_href(path: &str, query: &str, name: &str, anchor: &str) -> String {
    let separator = if query.is_empty() { '?' } else { '&' };
    format!("{path}{query}{separator}sort={name}{anchor}")
}
