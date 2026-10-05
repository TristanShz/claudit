//! Session export: one session as Markdown, to read or to paste into a
//! conversation with an AI (to improve the skills it used, say).
//!
//! - **Summary**: the session's general statistics, as on its dashboard
//!   page: header and totals, where the time went, skills, subagents,
//!   activities, tools, Bash commands and the list of turns.
//! - **Full**: the summary, then every turn in detail: its whole prompt,
//!   its time split and its trace (every tool call, main thread and
//!   subagents, with its input summary, wait, duration and error).
//!
//! Built from the stats API, so it holds what the archive holds: never
//! Claude's responses nor tool outputs, which claudit does not record.
//! Times are in the machine's time zone, as on the dashboard.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::{Result, bail};
use rusqlite::Connection;

use crate::activities::ActivityRules;
use crate::pricing::PriceTable;
use crate::stats::commands::{self, CommandSort};
use crate::stats::consumption::TokenTotals;
use crate::stats::prompt::PromptKind;
use crate::stats::session_detail::{self, SessionDetail};
use crate::stats::time::{SegmentKind, TimeSplit, TurnTime};
use crate::stats::tools::CallStats;
use crate::stats::trace::{self, LaneKind, TurnRow, TurnTrace};
use crate::stats::{activities, skills};
use crate::web::format;

/// Bash commands listed in an export, most time first.
const TOP_COMMANDS: usize = 20;

/// Longest call input in a full export's traces, in characters.
const INPUT_MAX: usize = 200;

/// How much of a session an export holds (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportLevel {
    Summary,
    Full,
}

impl ExportLevel {
    /// `summary` or `full` (file names, URLs).
    pub fn name(self) -> &'static str {
        match self {
            ExportLevel::Summary => "summary",
            ExportLevel::Full => "full",
        }
    }
}

/// The id of the one session whose id is `id_or_prefix` or starts with it.
pub fn resolve_session(conn: &Connection, id_or_prefix: &str) -> Result<String> {
    if id_or_prefix.is_empty() {
        bail!("no session id given");
    }
    let mut stmt = conn.prepare(
        "SELECT session_id FROM (SELECT session_id FROM sessions
                                 UNION SELECT session_id FROM turns)
         WHERE substr(session_id, 1, length(?1)) = ?1
         ORDER BY session_id = ?1 DESC, session_id LIMIT 4",
    )?;
    let ids = stmt
        .query_map([id_or_prefix], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    match ids.as_slice() {
        [] => bail!("no session matches `{id_or_prefix}`"),
        [first, ..] if first == id_or_prefix => Ok(first.clone()),
        [only] => Ok(only.clone()),
        several => bail!(
            "`{id_or_prefix}` matches several sessions ({}{}); give more of the id",
            several[..several.len().min(3)].join(", "),
            if several.len() > 3 { ", …" } else { "" }
        ),
    }
}

/// The export of `session_id` at `level`, `None` when nothing is known
/// about it.
pub fn session_markdown(
    conn: &Connection,
    session_id: &str,
    level: ExportLevel,
    rules: &ActivityRules,
    prices: &PriceTable,
) -> Result<Option<String>> {
    let Some(detail) = session_detail::session_detail(conn, session_id, prices)? else {
        return Ok(None);
    };
    let turns = trace::session_turns(conn, session_id, rules, prices)?;
    let times: HashMap<&str, &TurnTime> = detail
        .turns
        .iter()
        .map(|t| (t.prompt_id.as_str(), t))
        .collect();
    // Subagent runs are named `S1`, `S2`, … in the Subagents table and in
    // the traces, which then stay short.
    let run_ids: HashMap<&str, String> = detail
        .subagents
        .iter()
        .enumerate()
        .map(|(i, run)| (run.agent_id.as_str(), format!("S{}", i + 1)))
        .collect();

    let mut md = String::new();
    header(&mut md, &detail, level);
    time_section(&mut md, &detail);
    skills_section(&mut md, conn, session_id)?;
    subagents_section(&mut md, &detail, &turns, &run_ids);
    activities_section(&mut md, conn, session_id, rules)?;
    tools_section(&mut md, &detail);
    commands_section(&mut md, conn, session_id, rules)?;
    turns_section(&mut md, &turns, &times);
    if level == ExportLevel::Full {
        md.push_str("\n## Turn details\n");
        for turn in &turns {
            let Some(trace) = trace::turn_trace(conn, session_id, &turn.prompt_id, rules, prices)?
            else {
                continue;
            };
            let prompt = trace::turn_prompt_text(conn, session_id, &turn.prompt_id)?;
            turn_detail(
                &mut md,
                &trace,
                prompt.as_deref(),
                times.get(turn.prompt_id.as_str()),
                &run_ids,
            );
        }
    }
    Ok(Some(md))
}

fn header(md: &mut String, detail: &SessionDetail, level: ExportLevel) {
    let _ = writeln!(md, "# Claude Code session {}\n", detail.session_id);
    let offset = detail
        .started_at
        .map(|at| at.with_timezone(&chrono::Local).format("%:z").to_string())
        .unwrap_or_default();
    let _ = writeln!(
        md,
        "> {} export by claudit {}. Times are local (UTC{offset}). Claude's \
         responses and tool outputs are never recorded, so they are not here.",
        match level {
            ExportLevel::Summary => "Summary",
            ExportLevel::Full => "Full",
        },
        env!("CARGO_PKG_VERSION"),
    );

    md.push_str("\n## Session\n\n");
    let mut fact = |label: &str, value: String| {
        if !value.is_empty() {
            let _ = writeln!(md, "- **{label}**: {value}");
        }
    };
    fact(
        "First prompt",
        detail
            .first_prompt
            .as_deref()
            .map(|p| format::truncate(p, 500))
            .unwrap_or_default(),
    );
    if let Some(cwd) = &detail.cwd {
        let project = cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(cwd);
        fact("Project", format!("{project} (`{cwd}`)"));
    }
    fact("Branch", detail.git_branch.clone().unwrap_or_default());
    fact("Model", detail.model.clone().unwrap_or_default());
    fact("Claude Code", detail.version.clone().unwrap_or_default());
    fact(
        "Time",
        match (detail.started_at, detail.last_activity_at) {
            (Some(start), Some(end)) => {
                format!(
                    "{} → {}",
                    format::local_time(start),
                    format::local_time(end)
                )
            }
            (Some(start), None) => format::local_time(start),
            _ => String::new(),
        },
    );
    fact(
        "Turns",
        format!(
            "{} ({} timed by the hooks)",
            detail.turn_count,
            detail.turns.len()
        ),
    );
    fact(
        "Active time",
        if detail.imported {
            "not measured (imported from transcripts before claudit was installed)".to_owned()
        } else {
            format::duration(detail.time.wall())
        },
    );
    fact("Tokens", tokens_long(&detail.tokens));
    fact(
        "Cost",
        format!("{} (API list prices)", format::cost(&detail.cost)),
    );
    let calls: u64 = detail.tools.iter().map(|t| t.stats.calls).sum();
    let failures: u64 = detail.tools.iter().map(|t| t.stats.failures).sum();
    fact(
        "Tool calls",
        format!("{calls} ({failures} failed), subagents' included"),
    );
}

fn time_section(md: &mut String, detail: &SessionDetail) {
    if detail.imported || detail.turns.is_empty() {
        return;
    }
    md.push_str("\n## Where the time goes\n\n");
    let split: &TimeSplit = &detail.time;
    let wall = split.wall().num_milliseconds();
    let rows = SegmentKind::ALL.into_iter().map(|kind| {
        let d = split.component(kind);
        vec![
            kind.label().to_owned(),
            format::duration(d),
            if wall > 0 {
                format::percent(d.num_milliseconds() as f64 / wall as f64)
            } else {
                String::new()
            },
            match kind {
                SegmentKind::Model => "the model generating, and anything not otherwise explained",
                SegmentKind::Tool => "main-thread tools running (parallel calls counted once)",
                SegmentKind::Waiting => "tool calls waiting on a permission prompt",
                SegmentKind::Subagent => "the main thread blocked on a subagent",
                SegmentKind::Background => "subagents running between turns, the main thread idle",
            }
            .to_owned(),
        ]
    });
    table(md, &["Component", "Time", "Share", "What it is"], rows);
}

fn skills_section(md: &mut String, conn: &Connection, session_id: &str) -> Result<()> {
    let skills = skills::session_skill_ranking(conn, session_id)?;
    if skills.is_empty() {
        return Ok(());
    }
    md.push_str(
        "\n## Skills\n\nAttributed time: from each main-thread invocation to the next skill \
         of its turn, or the end of the turn. Tokens: API responses Claude Code \
         attributes to the skill, subagents included.\n\n",
    );
    let rows = skills.iter().map(|s| {
        vec![
            s.skill.clone(),
            s.user_invocations.to_string(),
            s.model_invocations.to_string(),
            format::duration(s.attributed_time),
            tokens_short(&s.tokens),
        ]
    });
    table(
        md,
        &["Skill", "Typed", "By Claude", "Attributed time", "Tokens"],
        rows,
    );
    Ok(())
}

fn subagents_section(
    md: &mut String,
    detail: &SessionDetail,
    turns: &[TurnRow],
    run_ids: &HashMap<&str, String>,
) {
    if detail.subagents.is_empty() {
        return;
    }
    md.push_str("\n## Subagents\n\n");
    let number: HashMap<&str, usize> = turns
        .iter()
        .map(|t| (t.prompt_id.as_str(), t.number))
        .collect();
    let rows = detail.subagents.iter().map(|run| {
        vec![
            run_ids
                .get(run.agent_id.as_str())
                .cloned()
                .unwrap_or_default(),
            run.prompt_id
                .as_deref()
                .and_then(|p| number.get(p))
                .map_or_else(|| "–".to_owned(), |n| n.to_string()),
            run.agent_type.clone(),
            run.description.clone().unwrap_or_default(),
            run.model.clone().unwrap_or_default(),
            run.duration
                .map_or_else(|| "–".to_owned(), format::duration),
            run.tool_calls.to_string(),
            tokens_short(&run.tokens),
        ]
    });
    table(
        md,
        &[
            "#",
            "Turn",
            "Type",
            "Description",
            "Model",
            "Active time",
            "Tool calls",
            "Tokens",
        ],
        rows,
    );
}

fn activities_section(
    md: &mut String,
    conn: &Connection,
    session_id: &str,
    rules: &ActivityRules,
) -> Result<()> {
    let breakdown = activities::session_activities(conn, session_id, rules)?;
    if breakdown.activities.is_empty() {
        return Ok(());
    }
    md.push_str(
        "\n## Activities\n\nEach call's execution time, summed: parallel calls add up, and \
         a subagent's calls count here as well as in its Agent call.\n\n",
    );
    let rows = breakdown.activities.iter().map(|a| {
        let top = a
            .top
            .iter()
            .take(3)
            .map(|d| format!("{} ({})", d.name, d.stats.calls))
            .collect::<Vec<_>>()
            .join(", ");
        vec![
            a.activity.clone(),
            a.stats.calls.to_string(),
            a.stats.failures.to_string(),
            format::duration_ms(a.stats.total_duration_ms),
            top,
        ]
    });
    table(
        md,
        &["Activity", "Calls", "Failed", "Time", "Most frequent"],
        rows,
    );
    Ok(())
}

fn tools_section(md: &mut String, detail: &SessionDetail) {
    if detail.tools.is_empty() {
        return;
    }
    md.push_str("\n## Tools\n\n");
    let rows = detail.tools.iter().map(|t| {
        let mut row = vec![t.name.clone()];
        row.extend(call_stats(&t.stats));
        row
    });
    table(
        md,
        &["Tool", "Calls", "Failed", "Total time", "Median", "p95"],
        rows,
    );
}

fn commands_section(
    md: &mut String,
    conn: &Connection,
    session_id: &str,
    rules: &ActivityRules,
) -> Result<()> {
    let ranking = commands::session_commands(conn, session_id, rules, CommandSort::Total)?;
    if ranking.commands.is_empty() {
        return Ok(());
    }
    md.push_str("\n## Bash commands\n\n");
    let rows = ranking.commands.iter().take(TOP_COMMANDS).map(|c| {
        let mut row = vec![c.command.clone(), c.activity.clone()];
        row.extend(call_stats(&c.stats));
        row
    });
    table(
        md,
        &[
            "Command",
            "Activity",
            "Runs",
            "Failed",
            "Total time",
            "Median",
            "p95",
        ],
        rows,
    );
    if ranking.commands.len() > TOP_COMMANDS {
        let _ = writeln!(
            md,
            "\n{} more commands not listed.",
            ranking.commands.len() - TOP_COMMANDS
        );
    }
    Ok(())
}

fn turns_section(md: &mut String, turns: &[TurnRow], times: &HashMap<&str, &TurnTime>) {
    if turns.is_empty() {
        return;
    }
    md.push_str(
        "\n## Turns\n\nDuration: from the prompt to Claude handing back. Background: \
         subagents still running after it, the main thread idle.\n\n",
    );
    let rows = turns.iter().map(|t| {
        let background = times
            .get(t.prompt_id.as_str())
            .map(|time| time.split.background)
            .filter(|d| *d > chrono::Duration::zero());
        vec![
            t.number.to_string(),
            t.started_at.map(format::local_time).unwrap_or_default(),
            t.prompt
                .as_ref()
                .map(|p| format!("{}{}", kind_tag(p.kind), format::truncate(&p.text, 120)))
                .unwrap_or_default(),
            t.duration.map_or_else(|| "–".to_owned(), format::duration),
            background.map_or_else(|| "–".to_owned(), format::duration),
            format!("{} ({} failed)", t.tool_calls, t.failed_calls),
            t.subagent_runs.to_string(),
            format!("{} ({} failed)", t.test_runs, t.failed_test_runs),
            tokens_short(&t.tokens),
            format::cost(&t.cost),
        ]
    });
    table(
        md,
        &[
            "#",
            "Started",
            "Prompt",
            "Duration",
            "Background",
            "Tool calls",
            "Subagents",
            "Test runs",
            "Tokens",
            "Cost",
        ],
        rows,
    );
}

fn turn_detail(
    md: &mut String,
    trace: &TurnTrace,
    prompt: Option<&str>,
    time: Option<&&TurnTime>,
    run_ids: &HashMap<&str, String>,
) {
    let turn = &trace.turn;
    let _ = write!(md, "\n### Turn {}", turn.number);
    if let Some(at) = turn.started_at {
        let _ = write!(md, " · {}", format::local_time_s(at));
    }
    if let Some(d) = turn.duration {
        let _ = write!(md, " · {}", format::duration(d));
    }
    md.push_str("\n\n");

    if let Some(label) = &turn.prompt {
        let _ = writeln!(md, "Prompt ({}):\n", kind_name(label.kind));
    }
    if let Some(text) = prompt.or(turn.prompt.as_ref().map(|p| p.text.as_str())) {
        fenced(md, text);
    }
    if let Some(time) = time {
        let parts: Vec<String> = SegmentKind::ALL
            .into_iter()
            .map(|kind| (kind, time.split.component(kind)))
            .filter(|(_, d)| *d > chrono::Duration::zero())
            .map(|(kind, d)| format!("{} {}", kind.label().to_lowercase(), format::duration(d)))
            .collect();
        if !parts.is_empty() {
            let _ = writeln!(md, "\nTime: {}.", parts.join(" · "));
        }
    }
    let _ = writeln!(
        md,
        "\nTokens: {} · cost {}.",
        tokens_long(&turn.tokens),
        format::cost(&turn.cost)
    );

    // The trace names each thread `main` or by its run's `S<n>`.
    let threads: Vec<String> = trace
        .lanes
        .iter()
        .map(|lane| match &lane.kind {
            LaneKind::Main => "main".to_owned(),
            LaneKind::Subagent { agent_id, .. } => run_ids
                .get(agent_id.as_str())
                .cloned()
                .unwrap_or_else(|| lane_name(&lane.kind)),
        })
        .collect();
    let subagents: Vec<String> = trace
        .lanes
        .iter()
        .zip(&threads)
        .filter_map(|(lane, thread)| match &lane.kind {
            LaneKind::Main => None,
            LaneKind::Subagent { model, .. } => {
                let active = lane
                    .spans
                    .iter()
                    .fold(chrono::Duration::zero(), |sum, (a, b)| sum + (*b - *a));
                let mut line = format!("- {thread}: {}", lane_name(&lane.kind));
                if let Some(model) = model {
                    let _ = write!(line, " ({model})");
                }
                if !lane.spans.is_empty() {
                    let _ = write!(line, ", active {}", format::duration(active));
                }
                Some(line)
            }
        })
        .collect();
    if !subagents.is_empty() {
        let _ = writeln!(md, "\nSubagents in this turn:\n\n{}", subagents.join("\n"));
    }

    if trace.calls.is_empty() {
        md.push_str("\nNo tool call.\n");
        return;
    }
    md.push('\n');
    let rows = trace.calls.iter().map(|call| {
        vec![
            call.launched_at
                .with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string(),
            threads.get(call.lane).cloned().unwrap_or_default(),
            call.tool_name.clone(),
            call.activity.clone(),
            format::truncate(&call.summary, INPUT_MAX),
            call.wait.map(format::duration).unwrap_or_default(),
            match (call.exec_start, call.exec_end) {
                (Some(a), Some(b)) => format::duration(b - a),
                _ => String::new(),
            },
            match call.success {
                Some(true) => "ok".to_owned(),
                Some(false) => match &call.error {
                    Some(error) => format!("failed: {}", format::truncate(error, 300)),
                    None => "failed".to_owned(),
                },
                None => "–".to_owned(),
            },
        ]
    });
    table(
        md,
        &[
            "At", "Thread", "Tool", "Activity", "Input", "Wait", "Duration", "Result",
        ],
        rows,
    );
}

/// Calls, failures, total, median and p95 cells.
fn call_stats(stats: &CallStats) -> [String; 5] {
    let ms = |v: Option<u64>| v.map_or_else(|| "–".to_owned(), format::duration_ms);
    [
        stats.calls.to_string(),
        stats.failures.to_string(),
        if stats.timed_calls > 0 {
            format::duration_ms(stats.total_duration_ms)
        } else {
            "–".to_owned()
        },
        ms(stats.median_duration_ms),
        ms(stats.p95_duration_ms),
    ]
}

fn tokens_short(tokens: &TokenTotals) -> String {
    format::count(tokens.total())
}

fn tokens_long(tokens: &TokenTotals) -> String {
    format!(
        "{} (input {}, output {}, cache write {}, cache read {})",
        format::count(tokens.total()),
        format::count(tokens.input),
        format::count(tokens.output),
        format::count(tokens.cache_write),
        format::count(tokens.cache_read)
    )
}

fn kind_name(kind: PromptKind) -> &'static str {
    match kind {
        PromptKind::Typed => "typed",
        PromptKind::Command => "command",
        PromptKind::SubagentMessage => "subagent message",
        PromptKind::TaskNotification => "task notification",
        PromptKind::LocalOutput => "local command output",
        PromptKind::Scheduled => "scheduled task",
    }
}

/// A prefix marking prompts Claude Code injected, empty for the user's.
fn kind_tag(kind: PromptKind) -> String {
    if kind.injected() {
        format!("[{}] ", kind_name(kind))
    } else {
        String::new()
    }
}

/// `main`, or a subagent's type and description.
fn lane_name(kind: &LaneKind) -> String {
    match kind {
        LaneKind::Main => "main".to_owned(),
        LaneKind::Subagent {
            agent_id,
            agent_type,
            description,
            ..
        } => match (agent_type, description) {
            (Some(t), Some(d)) => format!("{t}: {d}"),
            (Some(t), None) => t.clone(),
            (None, Some(d)) => d.clone(),
            (None, None) => format!("subagent {agent_id}"),
        },
    }
}

/// A Markdown table; cells are put on one line and their pipes escaped.
fn table(md: &mut String, headers: &[&str], rows: impl Iterator<Item = Vec<String>>) {
    let _ = writeln!(md, "| {} |", headers.join(" | "));
    let _ = writeln!(md, "|{}", " --- |".repeat(headers.len()));
    for row in rows {
        let cells: Vec<String> = row.iter().map(|c| cell(c)).collect();
        let _ = writeln!(md, "| {} |", cells.join(" | "));
    }
}

fn cell(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

/// `text` verbatim in a fenced block longer than any backtick run in it.
fn fenced(md: &mut String, text: &str) {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let _ = writeln!(md, "{fence}text\n{}\n{fence}", text.trim_end());
}
