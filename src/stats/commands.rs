//! Bash commands: which commands Claude runs most, and which take the most
//! time, by **command key** (`pnpm exec vitest`, `cargo test`,
//! `git status`; see [`crate::shell::command_key`]), each with its
//! activity.
//!
//! A Bash call is named after the detail [`ActivityRules::classify`] gives
//! it, so the key of a chain (`cd web && pnpm test | tail`) is that of its
//! most significant simple command: the one the matching activity rule
//! matched, else the one the line leads with (`cd`, `export`, `echo`, …
//! skipped). Calls are classified at query time, like activities.
//!
//! Every completed Bash call in the filter counts as a run (subagents'
//! included), successful or failed, imported sessions' included. Durations,
//! their nearest-rank median and p95 (as in [`super::tools`]) and shares
//! come from the runs the hooks timed only (`timed_calls` of `calls`): a
//! transcript's tool_use → tool_result span includes permission prompts.
//! A call whose command line is unknown has no key and is left out.
//!
//! Filters: as [`super::tools`] (completion time, the call's own `cwd`, its
//! session's branch, the model its turn or subagent thread first called).

use std::cmp::Reverse;
use std::collections::HashMap;

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use serde::Serialize;

use super::Filter;
use super::tools::{CallStats, TOOL_CALL_COLUMNS};
use crate::activities::{ActivityRules, ToolCall};

/// How a [`CommandRanking`] is ordered. Every order is descending, then by
/// runs, then total time (both descending), then command key; commands
/// without a timed run come last in the median and p95 orders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CommandSort {
    /// Most runs first.
    #[default]
    Calls,
    /// Most total (hook-timed) time first.
    Total,
    /// Slowest median run first.
    Median,
    /// Slowest 95th-percentile run first.
    P95,
    /// Most failed runs first.
    Failures,
}

/// One command key over the report's runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandStat {
    /// The command key, e.g. `pnpm exec vitest`.
    pub command: String,
    /// The activity of its runs (of its first run classified, should rules
    /// give runs of one key different activities).
    pub activity: String,
    pub stats: CallStats,
    /// Its share of the report's hook-timed Bash time, in `[0, 1]`.
    pub share_of_bash_time: f64,
}

/// Bash runs by command key.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CommandRanking {
    pub commands: Vec<CommandStat>,
    /// Runs in the report.
    pub calls: u64,
    /// Runs the hooks timed (durations are measured on these).
    pub timed_calls: u64,
    /// Summed execution time of the timed runs.
    pub total_duration_ms: u64,
}

impl CommandRanking {
    /// Reorders the commands.
    pub fn sort(&mut self, by: CommandSort) {
        // `None` (no timed run) sorts below every duration.
        let primary = |s: &CallStats| -> Option<u64> {
            match by {
                CommandSort::Calls => Some(s.calls),
                CommandSort::Total => Some(s.total_duration_ms),
                CommandSort::Median => s.median_duration_ms,
                CommandSort::P95 => s.p95_duration_ms,
                CommandSort::Failures => Some(s.failures),
            }
        };
        self.commands.sort_by(|a, b| {
            let rank = |c: &CommandStat| {
                (
                    Reverse(primary(&c.stats)),
                    Reverse(c.stats.calls),
                    Reverse(c.stats.total_duration_ms),
                )
            };
            rank(a)
                .cmp(&rank(b))
                .then_with(|| a.command.cmp(&b.command))
        });
    }
}

/// Bash runs by command key over the filtered calls, ordered by `sort`.
pub fn command_ranking(
    conn: &Connection,
    filter: &Filter,
    rules: &ActivityRules,
    sort: CommandSort,
) -> Result<CommandRanking> {
    let where_ = filter.sql(&TOOL_CALL_COLUMNS)?;
    ranking_where(conn, rules, sort, &where_.clause, where_.params)
}

/// Bash runs by command key in one session, its subagents' included,
/// unfiltered (the session page shows a session whole).
pub fn session_commands(
    conn: &Connection,
    session_id: &str,
    rules: &ActivityRules,
    sort: CommandSort,
) -> Result<CommandRanking> {
    ranking_where(
        conn,
        rules,
        sort,
        "tc.session_id = ?",
        vec![Value::Text(session_id.to_owned())],
    )
}

#[derive(Default)]
struct Group {
    activity: String,
    calls: u64,
    failures: u64,
    durations: Vec<u64>,
}

fn ranking_where(
    conn: &Connection,
    rules: &ActivityRules,
    sort: CommandSort,
    clause: &str,
    params: Vec<Value>,
) -> Result<CommandRanking> {
    let sql = format!(
        "SELECT tc.bash_command,
                CASE WHEN json_valid(tc.tool_input)
                     THEN json_extract(tc.tool_input, '$.command') END,
                tc.success, tc.hook_duration_ms
         FROM tool_calls tc LEFT JOIN sessions s ON s.session_id = tc.session_id
         WHERE tc.post_at_us IS NOT NULL AND tc.tool_name = 'Bash' AND {clause}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;

    // The same command lines recur: classify each distinct one once.
    let mut known: HashMap<(Option<String>, Option<String>), (String, String)> = HashMap::new();
    let mut groups: HashMap<String, Group> = HashMap::new();
    while let Some(row) = rows.next()? {
        let bash_command: Option<String> = row.get(0)?;
        // `command` may be absent or not a string.
        let command: Option<String> = row.get::<_, Option<String>>(1).unwrap_or(None);
        // No command at all (an empty or comment-only line): no key.
        if bash_command.is_none()
            && command
                .as_deref()
                .and_then(crate::shell::leading_command)
                .is_none()
        {
            continue;
        }
        let failed = row.get::<_, Option<i64>>(2)? == Some(0);
        let duration_ms = row.get::<_, Option<i64>>(3)?.map(|ms| ms.max(0) as u64);

        let (key, activity) =
            known
                .entry((bash_command, command))
                .or_insert_with_key(|(bash_command, command)| {
                    let c = rules.classify(&ToolCall {
                        tool_name: "Bash",
                        mcp_server: None,
                        bash_command: bash_command.as_deref(),
                        command: command.as_deref(),
                    });
                    (c.detail.into_owned(), c.activity.to_owned())
                });
        let group = groups.entry(key.clone()).or_insert_with(|| Group {
            activity: activity.clone(),
            ..Group::default()
        });
        group.calls += 1;
        group.failures += u64::from(failed);
        group.durations.extend(duration_ms);
    }

    let total_duration_ms: u64 = groups.values().flat_map(|g| &g.durations).sum();
    let mut ranking = CommandRanking {
        calls: groups.values().map(|g| g.calls).sum(),
        timed_calls: groups.values().map(|g| g.durations.len() as u64).sum(),
        total_duration_ms,
        commands: groups
            .into_iter()
            .map(|(command, g)| {
                let stats = CallStats::of(g.calls, g.failures, g.durations);
                CommandStat {
                    share_of_bash_time: if total_duration_ms == 0 {
                        0.0
                    } else {
                        stats.total_duration_ms as f64 / total_duration_ms as f64
                    },
                    command,
                    activity: g.activity,
                    stats,
                }
            })
            .collect(),
    };
    ranking.sort(sort);
    Ok(ranking)
}
