//! Table rows shared by several pages, formatted for display.

use std::collections::HashMap;

use chrono::Duration;
use serde::Serialize;

use super::format;
use crate::pricing::Cost;
use crate::stats::activities::ActivityBreakdown;
use crate::stats::models::ModelsReport;
use crate::stats::sessions::SessionSummary;
use crate::stats::skills::SkillStat;
use crate::stats::subagents::{SubagentRun, SubagentTypeStat};
use crate::stats::time::{SegmentKind, TimeSplit};
use crate::stats::tools::RankedCalls;

/// A tool, Bash command or MCP server.
#[derive(Serialize)]
pub(super) struct ToolRow {
    pub name: String,
    pub calls: u64,
    /// Calls the hooks timed (the durations are measured on these).
    pub timed_calls: u64,
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
            // No hook timed any of its calls (imported sessions): no time,
            // rather than a misleading 0.
            total: if stats.timed_calls == 0 {
                "–".to_owned()
            } else {
                format::duration_ms(stats.total_duration_ms)
            },
            median: optional(stats.median_duration_ms),
            p95: optional(stats.p95_duration_ms),
            failure_rate: format::percent(stats.failure_rate()),
            failures: stats.failures,
            name: row.name,
            calls: stats.calls,
            timed_calls: stats.timed_calls,
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
            // No run of the type was timed by the hooks (imported sessions).
            time: if s.total_duration.is_zero() {
                "–".to_owned()
            } else {
                format::duration(s.total_duration)
            },
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
    /// The Agent call's description (empty when unknown).
    pub description: String,
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
            duration: r.duration.map_or_else(|| "–".to_owned(), format::duration),
            tool_calls: r.tool_calls,
            model: r.model.unwrap_or_default(),
            tokens: format::count(r.tokens.total()),
            description: r.description.unwrap_or_default(),
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
    SegmentKind::ALL
        .into_iter()
        .map(|kind| (kind, split.component(kind)))
        .filter(|(_, d)| *d > Duration::zero())
        .map(|(kind, d)| BarPart {
            kind: kind.name(),
            label: kind.label(),
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
    /// Empty for an imported session.
    pub bar: Vec<BarPart>,
    /// Active time: the sum of its hook-timed turns' wall time; `–` for an
    /// imported session.
    pub duration: String,
    /// Known only from its transcripts (recorded before `claudit install`).
    pub imported: bool,
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
            let prompt = s.first_prompt.unwrap_or_default();
            SessionRow {
                started: format::local_time(s.started_at),
                project: project_name(&cwd),
                branch: s.git_branch.unwrap_or_default(),
                prompt: format::truncate(&prompt, 90),
                prompt_full: format::truncate(&prompt, 600),
                bar: if s.imported {
                    Vec::new()
                } else {
                    split_bar(&s.time)
                },
                duration: if s.imported {
                    "–".to_owned()
                } else {
                    format::duration(s.time.wall())
                },
                imported: s.imported,
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

/// A model (overview block and `/models`).
pub(super) struct ModelRow {
    pub model: String,
    pub sessions: u64,
    pub api_messages: u64,
    pub tokens: String,
    /// Share of all tokens, e.g. `42%`.
    pub token_share: String,
    /// The same share as a CSS width, e.g. `42.13`.
    pub token_pct: String,
    /// e.g. `80%`; empty without input tokens.
    pub cache_read_share: String,
    pub cost: String,
    /// Share of the priced cost, e.g. `61%`; `–` when unpriced.
    pub cost_share: String,
    pub main_messages: u64,
    pub main_tokens: String,
    pub main_cost: String,
    pub subagent_messages: u64,
    pub subagent_tokens: String,
    pub subagent_cost: String,
}

pub(super) fn model_rows(report: &ModelsReport) -> Vec<ModelRow> {
    let pct = |share: f64| format!("{:.0}%", share * 100.0);
    report
        .models
        .iter()
        .map(|m| {
            let token_share = report.token_share(m);
            ModelRow {
                model: if m.model.is_empty() {
                    "(no model)".to_owned()
                } else {
                    m.model.clone()
                },
                sessions: m.sessions,
                api_messages: m.api_messages,
                tokens: format::count(m.tokens.total()),
                token_share: pct(token_share),
                token_pct: format!("{:.2}", token_share * 100.0),
                cache_read_share: m.tokens.cache_read_share().map(pct).unwrap_or_default(),
                cost: format::cost(&m.cost),
                cost_share: report.cost_share(m).map_or_else(|| "–".to_owned(), pct),
                main_messages: m.main.api_messages,
                main_tokens: format::count(m.main.tokens.total()),
                main_cost: format::cost(&m.main.cost),
                subagent_messages: m.subagents.api_messages,
                subagent_tokens: format::count(m.subagents.tokens.total()),
                subagent_cost: format::cost(&m.subagents.cost),
            }
        })
        .collect()
}

/// An activity (overview, `/activities`, session page).
pub(super) struct ActivityRow {
    pub name: String,
    /// Palette slot: `var(--act-<color>)` (see [`activity_color`]).
    pub color: usize,
    /// Hook-timed execution time; `–` when no hook timed its calls.
    pub time: String,
    /// Share of the tool time, e.g. `42%` (`–` without time).
    pub share: String,
    /// Bar width relative to the largest activity, e.g. `61.50`: by time,
    /// or by calls when nothing in the report was hook-timed (see
    /// [`activities_by_calls`]).
    pub bar_pct: String,
    pub calls: u64,
    /// Calls the hooks timed (time, median and p95 are measured on these).
    pub timed_calls: u64,
    pub failures: u64,
    pub failure_rate: String,
    pub median: String,
    pub p95: String,
    /// Its top details (commands, tools, MCP servers).
    pub top: Vec<ToolRow>,
    /// Details beyond `top`.
    pub more_details: u64,
}

/// The built-in activities, in palette order.
const ACTIVITY_COLORS: [&str; 17] = [
    "Tests",
    "Build & typecheck",
    "Lint & format",
    "Git & GitHub",
    "Dependencies",
    "Run & scripts",
    "Search code",
    "Read files",
    "Edit files",
    "Web",
    "Subagents",
    "Skills",
    "MCP",
    "Planning & todos",
    crate::activities::WAITING,
    crate::activities::OTHER_SHELL,
    crate::activities::OTHER,
];

/// A stable palette slot per activity name: the built-in activities have
/// their own, a user-defined one gets one from its name.
pub(super) fn activity_color(name: &str) -> usize {
    ACTIVITY_COLORS
        .iter()
        .position(|n| *n == name)
        .unwrap_or_else(|| {
            let hash = name
                .bytes()
                .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
            hash % (ACTIVITY_COLORS.len() - 3)
        })
}

/// Whether activity bars show calls rather than time: nothing in the
/// report was timed by the hooks (only imported sessions), so time bars
/// would all be empty.
pub(super) fn activities_by_calls(breakdown: &ActivityBreakdown) -> bool {
    breakdown.total_duration_ms == 0
}

pub(super) fn activity_rows(breakdown: &ActivityBreakdown) -> Vec<ActivityRow> {
    let by_calls = activities_by_calls(breakdown);
    let size = |stats: &crate::stats::tools::CallStats| {
        if by_calls {
            stats.calls
        } else {
            stats.total_duration_ms
        }
    };
    let max = breakdown
        .activities
        .iter()
        .map(|a| size(&a.stats))
        .max()
        .unwrap_or(0);
    breakdown
        .activities
        .iter()
        .map(|a| {
            let stats = &a.stats;
            let timed = stats.timed_calls > 0;
            let top = tool_rows(a.top.clone());
            ActivityRow {
                color: activity_color(&a.activity),
                name: a.activity.clone(),
                time: if timed {
                    format::duration_ms(stats.total_duration_ms)
                } else {
                    "–".to_owned()
                },
                share: if timed && !by_calls {
                    format!("{:.0}%", breakdown.share(a) * 100.0)
                } else {
                    "–".to_owned()
                },
                bar_pct: if max > 0 {
                    format!("{:.2}", size(stats) as f64 * 100.0 / max as f64)
                } else {
                    "0".to_owned()
                },
                calls: stats.calls,
                timed_calls: stats.timed_calls,
                failures: stats.failures,
                failure_rate: format::percent(stats.failure_rate()),
                median: stats
                    .median_duration_ms
                    .map_or_else(|| "–".to_owned(), format::duration_ms),
                p95: stats
                    .p95_duration_ms
                    .map_or_else(|| "–".to_owned(), format::duration_ms),
                more_details: a.details.saturating_sub(top.len() as u64),
                top,
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
