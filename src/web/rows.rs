//! Table rows shared by several pages, formatted for display.

use std::collections::HashMap;

use chrono::Duration;
use serde::Serialize;

use super::format;
use crate::pricing::Cost;
use crate::stats::sessions::SessionSummary;
use crate::stats::skills::SkillStat;
use crate::stats::subagents::{SubagentRun, SubagentTypeStat};
use crate::stats::time::TimeSplit;
use crate::stats::tools::RankedCalls;

/// A tool, Bash command or MCP server.
#[derive(Serialize)]
pub(super) struct ToolRow {
    pub name: String,
    pub calls: u64,
    pub total_ms: u64,
    pub total: String,
    pub median: String,
    pub p95: String,
    pub failures: u64,
    pub failure_rate: String,
}

impl From<RankedCalls> for ToolRow {
    fn from(row: RankedCalls) -> Self {
        let stats = row.stats;
        let optional = |ms: Option<u64>| ms.map_or_else(|| "–".to_owned(), format::duration_ms);
        Self {
            total: format::duration_ms(stats.total_duration_ms),
            median: optional(stats.median_duration_ms),
            p95: optional(stats.p95_duration_ms),
            failure_rate: format::percent(stats.failure_rate()),
            failures: stats.failures,
            name: row.name,
            calls: stats.calls,
            total_ms: stats.total_duration_ms,
        }
    }
}

pub(super) fn tool_rows(rows: Vec<RankedCalls>) -> Vec<ToolRow> {
    rows.into_iter().map(ToolRow::from).collect()
}

/// A skill.
pub(super) struct SkillRow {
    pub name: String,
    /// Who triggered it: `you`, `Claude` or `you + Claude` (empty when only
    /// tokens are attributed to it in range).
    pub pill: &'static str,
    /// CSS modifier for the pill: `user`, `model` or `both`.
    pub pill_kind: &'static str,
    pub invocations: u64,
    pub user_invocations: u64,
    pub model_invocations: u64,
    pub tokens: String,
    pub time: String,
}

pub(super) fn skill_rows(stats: Vec<SkillStat>) -> Vec<SkillRow> {
    stats
        .into_iter()
        .map(|s| {
            let (pill, pill_kind) = trigger_pill(s.user_invocations > 0, s.model_invocations > 0);
            SkillRow {
                invocations: s.invocations(),
                user_invocations: s.user_invocations,
                model_invocations: s.model_invocations,
                tokens: format::count(s.tokens.total()),
                time: format::duration(s.attributed_time),
                name: s.skill,
                pill,
                pill_kind,
            }
        })
        .collect()
}

/// The pill text and CSS modifier for who triggered a skill.
pub(super) fn trigger_pill(by_user: bool, by_model: bool) -> (&'static str, &'static str) {
    match (by_user, by_model) {
        (true, true) => ("you + Claude", "both"),
        (true, false) => ("you", "user"),
        (false, true) => ("Claude", "model"),
        (false, false) => ("", ""),
    }
}

/// A subagent type.
pub(super) struct SubagentRow {
    pub agent_type: String,
    pub runs: u64,
    pub time: String,
    pub tool_calls: u64,
    pub model: String,
    pub tokens: String,
}

pub(super) fn subagent_rows(stats: Vec<SubagentTypeStat>) -> Vec<SubagentRow> {
    stats
        .into_iter()
        .map(|s| SubagentRow {
            runs: s.runs,
            time: format::duration(s.total_duration),
            tool_calls: s.tool_calls,
            model: s.model.unwrap_or_default(),
            tokens: format::count(s.tokens.total()),
            agent_type: s.agent_type,
        })
        .collect()
}

/// One subagent run.
pub(super) struct RunRow {
    pub agent_type: String,
    pub session_id: String,
    pub started: String,
    pub duration: String,
    pub tool_calls: u64,
    pub model: String,
    pub tokens: String,
}

pub(super) fn run_rows(runs: Vec<SubagentRun>) -> Vec<RunRow> {
    runs.into_iter()
        .map(|r| RunRow {
            started: r.started_at.map(format::local_time).unwrap_or_default(),
            duration: r.duration.map(format::duration).unwrap_or_default(),
            tool_calls: r.tool_calls,
            model: r.model.unwrap_or_default(),
            tokens: format::count(r.tokens.total()),
            agent_type: r.agent_type,
            session_id: r.session_id,
        })
        .collect()
}

/// One part of a split bar: a time component and its share of the whole.
pub(super) struct BarPart {
    /// `model`, `tool`, `waiting` or `subagent` (CSS hook).
    pub kind: &'static str,
    pub label: &'static str,
    /// Share of the total in percent, e.g. `42.5`.
    pub pct: String,
    pub value: String,
}

/// The four components of `split`, empty when it sums to zero.
pub(super) fn split_bar(split: &TimeSplit) -> Vec<BarPart> {
    let wall = split.wall().num_milliseconds();
    if wall <= 0 {
        return Vec::new();
    }
    [
        ("model", "Model", split.model),
        ("tool", "Tools", split.tool),
        ("waiting", "Waiting on you", split.waiting),
        ("subagent", "Subagents", split.subagent),
    ]
    .into_iter()
    .filter(|(_, _, d)| *d > Duration::zero())
    .map(|(kind, label, d)| BarPart {
        kind,
        label,
        pct: format!("{:.2}", d.num_milliseconds() as f64 * 100.0 / wall as f64),
        value: format::duration(d),
    })
    .collect()
}

/// A row of the session table.
pub(super) struct SessionRow {
    pub session_id: String,
    pub started: String,
    /// The project directory's last component.
    pub project: String,
    pub cwd: String,
    pub branch: String,
    pub prompt: String,
    pub prompt_full: String,
    pub bar: Vec<BarPart>,
    /// Active time: the sum of its turns' wall time.
    pub duration: String,
    pub tool_calls: u64,
    pub cost: String,
}

/// `costs` maps session ids to their cost (from `cost_by_session`).
pub(super) fn session_rows(
    sessions: Vec<SessionSummary>,
    costs: &HashMap<String, Cost>,
) -> Vec<SessionRow> {
    sessions
        .into_iter()
        .map(|s| {
            let cwd = s.cwd.unwrap_or_default();
            let prompt = format::prompt(&s.first_prompt.unwrap_or_default());
            SessionRow {
                started: format::local_time(s.started_at),
                project: project_name(&cwd),
                branch: s.git_branch.unwrap_or_default(),
                prompt: format::truncate(&prompt, 90),
                prompt_full: format::truncate(&prompt, 600),
                bar: split_bar(&s.time),
                duration: format::duration(s.time.wall()),
                tool_calls: s.tool_calls,
                cost: costs
                    .get(&s.session_id)
                    .map_or_else(|| "–".to_owned(), format::cost),
                cwd,
                session_id: s.session_id,
            }
        })
        .collect()
}

/// A project directory's last component (`acme-api`).
pub(super) fn project_name(cwd: &str) -> String {
    cwd.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}
