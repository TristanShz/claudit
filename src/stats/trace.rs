//! A session turn by turn: the list of its turns, and each turn's trace
//! (when every tool call and subagent ran). Not subject to filters: a
//! session is always shown whole.
//!
//! Timing follows the rest of the stats API: **durations come only from
//! hook data**. A turn's duration is `UserPromptSubmit` → `Stop`; a call's
//! execution is `[PostToolUse − duration_ms, PostToolUse]` and its
//! permission wait `PreToolUse` → execution start; a subagent lane's spans
//! are its `SubagentStart` → `SubagentStop` pairs. What only a transcript
//! saw gives counts and start instants (a turn's first entry, a call's
//! `tool_use` entry), never a length.

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::Value;

use super::consumption::TokenTotals;
use super::cost;
use super::prompt::{self, PromptLabel};
use super::subagents::{self, SubagentRun};
use crate::activities::{ActivityRules, ToolCall};
use crate::clock;
use crate::pricing::{Cost, PriceTable};

/// The built-in activity of test runs, counted per turn.
pub const TESTS: &str = "Tests";

/// Longest [`TraceCall::summary`], in characters.
pub const SUMMARY_MAX: usize = 300;

/// One turn of a session, timed or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnRow {
    pub prompt_id: String,
    /// 1-based position in the session, oldest first.
    pub number: usize,
    /// `UserPromptSubmit`, else the turn's first transcript entry (an
    /// instant only).
    pub started_at: Option<DateTime<Utc>>,
    /// `None` when neither the hooks nor the transcript had its text.
    pub prompt: Option<PromptLabel>,
    /// `UserPromptSubmit` → `Stop`; `None` for a turn the hooks did not
    /// time (never a transcript span).
    pub duration: Option<Duration>,
    /// Completed tool calls, subagents' included.
    pub tool_calls: u64,
    pub failed_calls: u64,
    /// Subagent runs it launched.
    pub subagent_runs: u64,
    /// Completed calls of activity [`TESTS`].
    pub test_runs: u64,
    pub failed_test_runs: u64,
    /// Its API responses, subagents' included.
    pub tokens: TokenTotals,
    pub cost: Cost,
}

/// A lane of a turn trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LaneKind {
    /// The main thread.
    Main,
    /// One subagent run.
    Subagent {
        agent_id: String,
        /// `None` for a thread no run describes.
        agent_type: Option<String>,
        model: Option<String>,
        /// The `description` of the Agent call that ran it.
        description: Option<String>,
    },
}

/// A swimlane: the main thread or one subagent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lane {
    pub kind: LaneKind,
    /// When it was active, hook-timed, oldest first: the turn's submit →
    /// stop for the main thread, start → stop pairs for a subagent (a run
    /// resumed later has several). Empty when the hooks did not time it.
    pub spans: Vec<(DateTime<Utc>, DateTime<Utc>)>,
}

/// One tool call of a turn trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraceCall {
    pub tool_use_id: String,
    /// Index into [`TurnTrace::lanes`].
    pub lane: usize,
    pub tool_name: String,
    pub activity: String,
    /// A one-line summary of its (redacted) input: the Bash command, the
    /// file path, the pattern, the URL, the MCP server and tool, the skill,
    /// the subagent type and description. Never its output.
    pub summary: String,
    /// When it was launched: `PreToolUse`, else the transcript's `tool_use`
    /// entry, else its execution start.
    pub launched_at: DateTime<Utc>,
    /// Hook-timed execution; `None` for a call only a transcript saw, or
    /// one that never completed.
    pub exec_start: Option<DateTime<Utc>>,
    pub exec_end: Option<DateTime<Utc>>,
    /// `PreToolUse` → execution start, when positive: a permission prompt.
    pub wait: Option<Duration>,
    /// `None` while no result is known.
    pub success: Option<bool>,
    /// The hook-reported error (redacted).
    pub error: Option<String>,
    /// The lane of the subagent run this Agent call launched.
    pub spawned_lane: Option<usize>,
}

/// When each tool call and subagent of a turn ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnTrace {
    pub turn: TurnRow,
    /// The span the trace covers: the turn, and everything launched in it
    /// (a background subagent may outlive the turn).
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// The main thread first, then subagents by first activity.
    pub lanes: Vec<Lane>,
    /// By launch time.
    pub calls: Vec<TraceCall>,
}

/// Every turn of `session_id`, oldest first (by `UserPromptSubmit`, else
/// first transcript entry).
pub fn session_turns(
    conn: &Connection,
    session_id: &str,
    rules: &ActivityRules,
    prices: &PriceTable,
) -> Result<Vec<TurnRow>> {
    let runs = subagents::session_subagent_runs(conn, session_id)?;
    turn_rows(conn, session_id, None, &runs, rules, prices)
}

/// The trace of turn `prompt_id` of `session_id`, `None` for an unknown
/// turn.
pub fn turn_trace(
    conn: &Connection,
    session_id: &str,
    prompt_id: &str,
    rules: &ActivityRules,
    prices: &PriceTable,
) -> Result<Option<TurnTrace>> {
    let runs = subagents::session_subagent_runs(conn, session_id)?;
    let Some(turn) = turn_rows(conn, session_id, Some(prompt_id), &runs, rules, prices)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let (submit, stop, first_entry, last_entry): (
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    ) = conn.query_row(
        "SELECT submit_at_us, stop_at_us, start_at_us, end_at_us FROM turns
             WHERE session_id = ?1 AND prompt_id = ?2",
        params![session_id, prompt_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let timed = matches!((submit, stop), (Some(a), Some(b)) if b >= a);
    let turn_start = submit.or(first_entry);
    let turn_end = if timed { stop } else { last_entry };

    let calls = load_calls(conn, session_id, prompt_id, rules)?;

    // Lanes: the main thread, then every run launched in the turn or making
    // calls in it, then runs merely active during it.
    let mut lanes = vec![Lane {
        kind: LaneKind::Main,
        spans: if timed {
            vec![(
                clock::from_micros(submit.unwrap_or_default()),
                clock::from_micros(stop.unwrap_or_default()),
            )]
        } else {
            Vec::new()
        },
    }];
    let mut lane_of: HashMap<String, usize> = HashMap::new();
    let by_id: HashMap<&str, &SubagentRun> =
        runs.iter().map(|r| (r.agent_id.as_str(), r)).collect();
    let mut owned: Vec<(i64, String)> = Vec::new();
    for run in &runs {
        if run.prompt_id.as_deref() == Some(prompt_id) {
            owned.push((first_instant(run, &calls), run.agent_id.clone()));
        }
    }
    for call in &calls {
        if let Some(agent) = &call.agent_id
            && !owned.iter().any(|(_, a)| a == agent)
        {
            let first = by_id
                .get(agent.as_str())
                .map_or(call.launched_us, |run| first_instant(run, &calls));
            owned.push((first, agent.clone()));
        }
    }
    owned.sort();
    for (_, agent) in owned {
        lane_of.insert(agent.clone(), lanes.len());
        lanes.push(subagent_lane(
            &agent,
            by_id.get(agent.as_str()).copied(),
            None,
        ));
    }

    // The window: the turn, its calls and its own runs' spans.
    let mut start = turn_start;
    let mut end = turn_end;
    let mut widen = |at: i64| {
        start = Some(start.map_or(at, |s| s.min(at)));
        end = Some(end.map_or(at, |e| e.max(at)));
    };
    for call in &calls {
        widen(call.launched_us);
        if let Some(post) = call.exec.map(|(_, post)| post) {
            widen(post);
        }
    }
    for lane in &lanes {
        for (a, b) in &lane.spans {
            widen(clock::to_micros(*a));
            widen(clock::to_micros(*b));
        }
    }
    let (Some(start), Some(end)) = (start, end) else {
        return Ok(Some(TurnTrace {
            turn,
            start: DateTime::<Utc>::default(),
            end: DateTime::<Utc>::default(),
            lanes,
            calls: Vec::new(),
        }));
    };

    // Runs of other turns active while this one ran, clipped to it.
    if let (true, Some(t0), Some(t1)) = (timed, turn_start, turn_end) {
        for run in &runs {
            if lane_of.contains_key(&run.agent_id) {
                continue;
            }
            let spans: Vec<_> = run
                .active
                .iter()
                .filter(|(a, b)| clock::to_micros(*a) < t1 && clock::to_micros(*b) > t0)
                .map(|(a, b)| {
                    (
                        clock::from_micros(clock::to_micros(*a).max(start)),
                        clock::from_micros(clock::to_micros(*b).min(end)),
                    )
                })
                .collect();
            if !spans.is_empty() {
                lane_of.insert(run.agent_id.clone(), lanes.len());
                lanes.push(subagent_lane(&run.agent_id, Some(run), Some(spans)));
            }
        }
    }

    let spawned: HashMap<&str, usize> = runs
        .iter()
        .filter_map(|r| {
            let lane = *lane_of.get(&r.agent_id)?;
            Some((r.parent_tool_use_id.as_deref()?, lane))
        })
        .collect();
    let calls = calls
        .into_iter()
        .map(|call| TraceCall {
            lane: call
                .agent_id
                .as_ref()
                .and_then(|a| lane_of.get(a).copied())
                .unwrap_or(0),
            spawned_lane: spawned.get(call.tool_use_id.as_str()).copied(),
            launched_at: clock::from_micros(call.launched_us),
            exec_start: call.exec.map(|(a, _)| clock::from_micros(a)),
            exec_end: call.exec.map(|(_, b)| clock::from_micros(b)),
            wait: call.wait_us.map(Duration::microseconds),
            tool_use_id: call.tool_use_id,
            tool_name: call.tool_name,
            activity: call.activity,
            summary: call.summary,
            success: call.success,
            error: call.error,
        })
        .collect();
    Ok(Some(TurnTrace {
        turn,
        start: clock::from_micros(start),
        end: clock::from_micros(end),
        lanes,
        calls,
    }))
}

/// A subagent's lane; `spans` overrides the run's own active spans.
fn subagent_lane(
    agent_id: &str,
    run: Option<&SubagentRun>,
    spans: Option<Vec<(DateTime<Utc>, DateTime<Utc>)>>,
) -> Lane {
    Lane {
        kind: LaneKind::Subagent {
            agent_id: agent_id.to_owned(),
            agent_type: run.map(|r| r.agent_type.clone()),
            model: run.and_then(|r| r.model.clone()),
            description: run.and_then(|r| r.description.clone()),
        },
        spans: spans.unwrap_or_else(|| run.map(|r| r.active.clone()).unwrap_or_default()),
    }
}

/// When a run first did something: its first active span, its first call
/// in `calls`, else its known start.
fn first_instant(run: &SubagentRun, calls: &[Call]) -> i64 {
    let span = run.active.first().map(|(a, _)| clock::to_micros(*a));
    let call = calls
        .iter()
        .filter(|c| c.agent_id.as_deref() == Some(run.agent_id.as_str()))
        .map(|c| c.launched_us)
        .min();
    [span, call, run.started_at.map(clock::to_micros)]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(i64::MAX)
}

/// A tool call as loaded, times in µs.
struct Call {
    tool_use_id: String,
    agent_id: Option<String>,
    tool_name: String,
    activity: String,
    summary: String,
    launched_us: i64,
    exec: Option<(i64, i64)>,
    wait_us: Option<i64>,
    success: Option<bool>,
    error: Option<String>,
}

/// The calls of one turn, by launch time.
fn load_calls(
    conn: &Connection,
    session_id: &str,
    prompt_id: &str,
    rules: &ActivityRules,
) -> Result<Vec<Call>> {
    let mut stmt = conn.prepare(
        "SELECT tool_use_id, agent_id, tool_name, mcp_server, bash_command, tool_input,
                hook_pre_at_us, hook_post_at_us, hook_duration_ms, transcript_pre_at_us,
                CASE WHEN post_at_us IS NOT NULL THEN success END, error
         FROM tool_calls
         WHERE session_id = ?1 AND prompt_id = ?2",
    )?;
    let mut classifier = Classifier::new(rules);
    let mut rows = stmt.query(params![session_id, prompt_id])?;
    let mut calls = Vec::new();
    while let Some(row) = rows.next()? {
        let tool_name: String = row.get(2)?;
        let mcp_server: Option<String> = row.get(3)?;
        let bash_command: Option<String> = row.get(4)?;
        let input: Value = row
            .get::<_, Option<String>>(5)?
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        let hook_pre: Option<i64> = row.get(6)?;
        let hook_post: Option<i64> = row.get(7)?;
        let hook_duration: Option<i64> = row.get(8)?;
        let transcript_pre: Option<i64> = row.get(9)?;

        let exec_start = match (hook_post, hook_duration) {
            (Some(post), Some(ms)) => Some(clock::execution_start_us(post, ms)),
            (Some(_), None) => hook_pre,
            _ => None,
        };
        let exec = exec_start.zip(hook_post);
        let wait_us = match (hook_pre, exec_start) {
            (Some(pre), Some(start)) if start > pre => Some(start - pre),
            _ => None,
        };
        let Some(launched_us) = hook_pre.or(transcript_pre).or(exec_start).or(hook_post) else {
            continue;
        };
        let command = input.get("command").and_then(Value::as_str);
        let activity = classifier.activity(
            &tool_name,
            mcp_server.as_deref(),
            bash_command.as_deref(),
            command,
        );
        calls.push(Call {
            tool_use_id: row.get(0)?,
            agent_id: row.get(1)?,
            summary: summary(&tool_name, mcp_server.as_deref(), &input),
            activity,
            tool_name,
            launched_us,
            exec,
            wait_us,
            success: row.get::<_, Option<i64>>(10)?.map(|s| s != 0),
            error: row.get(11)?,
        });
    }
    calls.sort_by(|a, b| {
        (a.launched_us, &a.agent_id, &a.tool_use_id).cmp(&(
            b.launched_us,
            &b.agent_id,
            &b.tool_use_id,
        ))
    });
    Ok(calls)
}

/// What classification reads of a call: tool, MCP server, leading
/// command, command line.
type CallKey = (String, Option<String>, Option<String>, Option<String>);

/// Classifies calls, each distinct one once.
struct Classifier<'r> {
    rules: &'r ActivityRules,
    known: HashMap<CallKey, String>,
}

impl<'r> Classifier<'r> {
    fn new(rules: &'r ActivityRules) -> Self {
        Self {
            rules,
            known: HashMap::new(),
        }
    }

    fn activity(
        &mut self,
        tool_name: &str,
        mcp_server: Option<&str>,
        bash_command: Option<&str>,
        command: Option<&str>,
    ) -> String {
        let key = (
            tool_name.to_owned(),
            mcp_server.map(str::to_owned),
            bash_command.map(str::to_owned),
            command.map(str::to_owned),
        );
        if let Some(activity) = self.known.get(&key) {
            return activity.clone();
        }
        let activity = self
            .rules
            .classify(&ToolCall {
                tool_name,
                mcp_server,
                bash_command,
                command,
            })
            .activity
            .to_owned();
        self.known.insert(key, activity.clone());
        activity
    }
}

/// The turns of a session (or only `only`), with their counts.
fn turn_rows(
    conn: &Connection,
    session_id: &str,
    only: Option<&str>,
    runs: &[SubagentRun],
    rules: &ActivityRules,
    prices: &PriceTable,
) -> Result<Vec<TurnRow>> {
    let names = subagents::display_names(runs);

    let mut stmt = conn.prepare(
        "SELECT prompt_id, prompt_text, submit_at_us, stop_at_us, start_at_us
         FROM turns WHERE session_id = ?1
         ORDER BY COALESCE(submit_at_us, start_at_us) IS NULL,
                  COALESCE(submit_at_us, start_at_us), prompt_id",
    )?;
    let mut rows = stmt.query([session_id])?;
    let mut turns: Vec<TurnRow> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut number = 0;
    while let Some(row) = rows.next()? {
        number += 1;
        let prompt_id: String = row.get(0)?;
        if only.is_some_and(|p| p != prompt_id) {
            continue;
        }
        let text: Option<String> = row.get(1)?;
        let submit: Option<i64> = row.get(2)?;
        let stop: Option<i64> = row.get(3)?;
        let first_entry: Option<i64> = row.get(4)?;
        index.insert(prompt_id.clone(), turns.len());
        turns.push(TurnRow {
            number,
            started_at: submit.or(first_entry).map(clock::from_micros),
            prompt: text.as_deref().map(|t| prompt::label(t, &names)),
            duration: match (submit, stop) {
                (Some(a), Some(b)) if b >= a => Some(Duration::microseconds(b - a)),
                _ => None,
            },
            tool_calls: 0,
            failed_calls: 0,
            subagent_runs: runs
                .iter()
                .filter(|r| r.prompt_id.as_deref() == Some(prompt_id.as_str()))
                .count() as u64,
            test_runs: 0,
            failed_test_runs: 0,
            tokens: TokenTotals::default(),
            cost: Cost::default(),
            prompt_id,
        });
    }

    let mut stmt = conn.prepare(
        "SELECT prompt_id, tool_name, mcp_server, bash_command,
                CASE WHEN tool_name = 'Bash' AND json_valid(tool_input)
                     THEN json_extract(tool_input, '$.command') END,
                success
         FROM tool_calls
         WHERE session_id = ?1 AND post_at_us IS NOT NULL AND prompt_id IS NOT NULL
           AND (?2 IS NULL OR prompt_id = ?2)",
    )?;
    let mut rows = stmt.query(params![session_id, only])?;
    let mut classifier = Classifier::new(rules);
    while let Some(row) = rows.next()? {
        let prompt_id: String = row.get(0)?;
        let Some(&i) = index.get(&prompt_id) else {
            continue;
        };
        let tool_name: String = row.get(1)?;
        let mcp_server: Option<String> = row.get(2)?;
        let bash_command: Option<String> = row.get(3)?;
        let command: Option<String> = row.get::<_, Option<String>>(4).unwrap_or(None);
        let failed = row.get::<_, Option<i64>>(5)? == Some(0);
        let turn = &mut turns[i];
        turn.tool_calls += 1;
        turn.failed_calls += u64::from(failed);
        if classifier.activity(
            &tool_name,
            mcp_server.as_deref(),
            bash_command.as_deref(),
            command.as_deref(),
        ) == TESTS
        {
            turn.test_runs += 1;
            turn.failed_test_runs += u64::from(failed);
        }
    }

    for line in cost::session_cost_by_turn(conn, session_id, prices)? {
        if let Some(&i) = index.get(&line.key) {
            turns[i].tokens = line.tokens;
            turns[i].cost = line.cost;
        }
    }
    Ok(turns)
}

/// A one-line summary of a call's input (see [`TraceCall::summary`]).
fn summary(tool_name: &str, mcp_server: Option<&str>, input: &Value) -> String {
    let field = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let joined = |parts: &[Option<&str>], sep: &str| {
        parts
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>()
            .join(sep)
    };
    let text = match tool_name {
        "Bash" => field("command").unwrap_or_default().to_owned(),
        "Read" | "Write" | "Edit" | "MultiEdit" => {
            field("file_path").unwrap_or_default().to_owned()
        }
        "NotebookEdit" | "NotebookRead" => field("notebook_path").unwrap_or_default().to_owned(),
        "Grep" | "Glob" => match (field("pattern"), field("path")) {
            (Some(pattern), Some(path)) => format!("{pattern} in {path}"),
            (pattern, _) => pattern.unwrap_or_default().to_owned(),
        },
        "WebFetch" => field("url").unwrap_or_default().to_owned(),
        "WebSearch" => field("query").unwrap_or_default().to_owned(),
        "Agent" | "Task" => joined(&[field("subagent_type"), field("description")], " · "),
        "Skill" => joined(&[field("skill"), field("args")], " "),
        "TaskOutput" | "TaskStop" => field("task_id").unwrap_or_default().to_owned(),
        _ => match mcp_server {
            Some(server) => {
                let tool = tool_name
                    .strip_prefix(&format!("mcp__{server}__"))
                    .unwrap_or(tool_name);
                format!("{server} · {tool}")
            }
            None => [
                "description",
                "command",
                "file_path",
                "path",
                "pattern",
                "url",
                "query",
            ]
            .into_iter()
            .find_map(field)
            .unwrap_or_default()
            .to_owned(),
        },
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(SUMMARY_MAX) {
        Some((at, _)) => format!("{}…", text[..at].trim_end()),
        None => text,
    }
}
