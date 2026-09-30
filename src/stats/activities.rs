//! Activities: what Claude's tool time is spent on (tests, builds, git, …).
//!
//! Every completed tool call in the filter (subagents' calls included) is
//! classified with an [`ActivityRules`] at query time, in Rust, so a rule
//! change applies to the whole archive without re-ingesting. Per activity:
//! calls, failures, execution time (hook-timed calls only, as in
//! [`super::tools`]: calls imported from transcripts count as calls only),
//! nearest-rank median and p95, and the top details (normalized Bash
//! commands, tools, MCP servers; see [`crate::activities`]).
//!
//! Tool time is summed per call, not unioned: parallel calls add up, and
//! an Agent call's time covers its subagent's own calls. Shares are of the
//! summed time of every call in the report.
//!
//! Filters: as [`super::tools`] (completion time, the call's own `cwd`,
//! its session's branch, the model its turn or subagent thread first
//! called).

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use serde::Serialize;

use super::Filter;
use super::tools::{self, CallStats, RankedCalls, TOOL_CALL_COLUMNS};
use crate::activities::{ActivityRules, ToolCall};

/// Details kept per activity in [`ActivityStat::top`].
pub const TOP_DETAILS: usize = 5;

/// One activity over the report's calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivityStat {
    pub activity: String,
    pub stats: CallStats,
    /// Its most frequent details (ranked as `tools::tool_ranking`), at most
    /// [`TOP_DETAILS`].
    pub top: Vec<RankedCalls>,
    /// How many distinct details it has in all.
    pub details: u64,
}

/// One activity's calls on one UTC day (of their completion).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivityDay {
    pub day: NaiveDate,
    pub activity: String,
    pub calls: u64,
    pub duration_ms: u64,
}

/// Tool time by activity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ActivityBreakdown {
    /// Most time first, then most calls, then name.
    pub activities: Vec<ActivityStat>,
    /// Summed execution time of every hook-timed call in the report.
    pub total_duration_ms: u64,
    /// By day, then activity name.
    pub by_day: Vec<ActivityDay>,
}

impl ActivityBreakdown {
    /// `activity`'s share of the report's tool time, in `[0, 1]`.
    pub fn share(&self, activity: &ActivityStat) -> f64 {
        if self.total_duration_ms == 0 {
            0.0
        } else {
            activity.stats.total_duration_ms as f64 / self.total_duration_ms as f64
        }
    }
}

/// Tool time by activity over the filtered calls.
pub fn activity_breakdown(
    conn: &Connection,
    filter: &Filter,
    rules: &ActivityRules,
) -> Result<ActivityBreakdown> {
    let where_ = filter.sql(&TOOL_CALL_COLUMNS)?;
    breakdown_where(conn, rules, &where_.clause, where_.params)
}

/// Tool time by activity in one session, its subagents' calls included,
/// unfiltered (the session page shows a session whole).
pub fn session_activities(
    conn: &Connection,
    session_id: &str,
    rules: &ActivityRules,
) -> Result<ActivityBreakdown> {
    breakdown_where(
        conn,
        rules,
        "tc.session_id = ?",
        vec![Value::Text(session_id.to_owned())],
    )
}

#[derive(Default)]
struct Group {
    calls: u64,
    failures: u64,
    durations: Vec<u64>,
}

impl Group {
    fn add(&mut self, failed: bool, duration_ms: Option<u64>) {
        self.calls += 1;
        self.failures += u64::from(failed);
        self.durations.extend(duration_ms);
    }
}

#[derive(Default)]
struct ActivityGroup {
    all: Group,
    details: HashMap<String, Group>,
}

fn breakdown_where(
    conn: &Connection,
    rules: &ActivityRules,
    clause: &str,
    params: Vec<Value>,
) -> Result<ActivityBreakdown> {
    let sql = format!(
        "SELECT tc.tool_name, tc.mcp_server, tc.bash_command,
                CASE WHEN tc.tool_name = 'Bash' AND json_valid(tc.tool_input)
                     THEN json_extract(tc.tool_input, '$.command') END,
                tc.success, tc.hook_duration_ms,
                date(tc.post_at_us / 1000000, 'unixepoch')
         FROM tool_calls tc LEFT JOIN sessions s ON s.session_id = tc.session_id
         WHERE tc.post_at_us IS NOT NULL AND {clause}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;

    let mut groups: HashMap<&str, ActivityGroup> = HashMap::new();
    let mut days: BTreeMap<(String, &str), (u64, u64)> = BTreeMap::new();
    type CallKey = (String, Option<String>, Option<String>, Option<String>);
    let mut known: HashMap<CallKey, (&str, String)> = HashMap::new();
    while let Some(row) = rows.next()? {
        let tool_name: String = row.get(0)?;
        let mcp_server: Option<String> = row.get(1)?;
        let bash_command: Option<String> = row.get(2)?;
        // `command` may be absent or not a string.
        let command: Option<String> = row.get::<_, Option<String>>(3).unwrap_or(None);
        let failed = row.get::<_, Option<i64>>(4)? == Some(0);
        let duration_ms = row.get::<_, Option<i64>>(5)?.map(|ms| ms.max(0) as u64);
        let day: String = row.get(6)?;

        // The same calls recur (`cargo test`, `Read`): classify each
        // distinct one once.
        let key = (tool_name, mcp_server, bash_command, command);
        let (activity, detail) = match known.get(&key) {
            Some(&(activity, ref detail)) => (activity, detail.as_str()),
            None => {
                let (tool_name, mcp_server, bash_command, command) = &key;
                let classification = rules.classify(&ToolCall {
                    tool_name,
                    mcp_server: mcp_server.as_deref(),
                    bash_command: bash_command.as_deref(),
                    command: command.as_deref(),
                });
                let value = (classification.activity, classification.detail.into_owned());
                let (activity, detail) = known.entry(key).or_insert(value);
                (*activity, detail.as_str())
            }
        };
        let group = groups.entry(activity).or_default();
        group.all.add(failed, duration_ms);
        match group.details.get_mut(detail) {
            Some(group) => group.add(failed, duration_ms),
            None => {
                let mut group_of_detail = Group::default();
                group_of_detail.add(failed, duration_ms);
                group.details.insert(detail.to_owned(), group_of_detail);
            }
        }
        let day_total = days.entry((day, activity)).or_default();
        day_total.0 += 1;
        day_total.1 += duration_ms.unwrap_or(0);
    }

    let mut activities: Vec<ActivityStat> = groups
        .into_iter()
        .map(|(activity, group)| {
            let details = group.details.len() as u64;
            let mut top: Vec<RankedCalls> = group
                .details
                .into_iter()
                .map(|(name, g)| RankedCalls {
                    name,
                    stats: CallStats::of(g.calls, g.failures, g.durations),
                })
                .collect();
            tools::sort_ranking(&mut top);
            top.truncate(TOP_DETAILS);
            ActivityStat {
                activity: activity.to_owned(),
                stats: CallStats::of(group.all.calls, group.all.failures, group.all.durations),
                top,
                details,
            }
        })
        .collect();
    activities.sort_by(|a, b| {
        (b.stats.total_duration_ms, b.stats.calls)
            .cmp(&(a.stats.total_duration_ms, a.stats.calls))
            .then_with(|| a.activity.cmp(&b.activity))
    });
    let by_day = days
        .into_iter()
        .map(|((day, activity), (calls, duration_ms))| {
            Ok(ActivityDay {
                day: NaiveDate::parse_from_str(&day, "%Y-%m-%d")?,
                activity: activity.to_owned(),
                calls,
                duration_ms,
            })
        })
        .collect::<Result<_>>()?;
    Ok(ActivityBreakdown {
        total_duration_ms: activities.iter().map(|a| a.stats.total_duration_ms).sum(),
        activities,
        by_day,
    })
}
