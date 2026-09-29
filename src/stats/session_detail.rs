//! One session in detail: header, totals, turn timeline, tools, skills and
//! subagents (the session page). Not subject to filters: a session is
//! always shown whole.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use super::consumption::TokenTotals;
use super::skills::{self, SkillInvocation};
use super::subagents::{self, SubagentRun};
use super::time::{self, TimeSplit, TurnTime};
use super::tools::{self, RankedCalls};
use crate::clock;
use crate::pricing::{Cost, PriceTable};

/// Everything the session page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDetail {
    pub session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    /// Claude Code version.
    pub version: Option<String>,
    /// First known activity (`None` for a session known only from tool
    /// calls whose hooks carried no session start).
    pub started_at: Option<DateTime<Utc>>,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub first_prompt: Option<String>,
    /// The main thread's model (the one most of its API responses used).
    pub model: Option<String>,
    /// Tokens of every API response, subagents included.
    pub tokens: TokenTotals,
    pub cost: Cost,
    /// Its turns' split, summed.
    pub time: TimeSplit,
    /// Main-thread turns, oldest first, with their positioned segments.
    pub turns: Vec<TurnTime>,
    /// Its tools, subagents' calls included (ranked as `tool_ranking`).
    pub tools: Vec<RankedCalls>,
    pub skills: Vec<SkillInvocation>,
    pub subagents: Vec<SubagentRun>,
}

/// The detail of `session_id`, `None` when nothing is known about it.
pub fn session_detail(
    conn: &Connection,
    session_id: &str,
    prices: &PriceTable,
) -> Result<Option<SessionDetail>> {
    /// `cwd, git_branch, version, first_at_us, last_at_us` of `sessions`.
    type Header = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
    );
    let header: Option<Header> = conn
        .query_row(
            "SELECT cwd, git_branch, version, first_at_us, last_at_us
             FROM sessions WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let turns = time::session_turn_times(conn, session_id)?;
    let tools = tools::session_tool_ranking(conn, session_id)?;
    let (mut cwd, git_branch, version, first_at, last_at) =
        header.unwrap_or((None, None, None, None, None));
    if first_at.is_none() && turns.is_empty() && tools.is_empty() {
        return Ok(None);
    }
    if cwd.is_none() {
        cwd = conn
            .query_row(
                "SELECT cwd FROM tool_calls WHERE session_id = ?1 AND cwd IS NOT NULL LIMIT 1",
                [session_id],
                |row| row.get(0),
            )
            .optional()?;
    }

    let first_prompt: Option<String> = conn
        .query_row(
            "SELECT prompt_text FROM turns
             WHERE session_id = ?1 AND prompt_text IS NOT NULL
             ORDER BY COALESCE(submit_at_us, start_at_us) LIMIT 1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;
    let model: Option<String> = conn
        .query_row(
            "SELECT model FROM api_messages
             WHERE session_id = ?1 AND agent_id IS NULL AND model IS NOT NULL
             GROUP BY model ORDER BY COUNT(*) DESC, model LIMIT 1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;

    let mut tokens = TokenTotals::default();
    let mut cost = Cost::default();
    let mut stmt = conn.prepare(&format!(
        "SELECT COALESCE(m.model, ''), COALESCE(SUM(m.cache_write_1h_tokens), 0), {}
         FROM api_messages m WHERE m.session_id = ?1
         GROUP BY COALESCE(m.model, '')",
        TokenTotals::SUMS
    ))?;
    let rows = stmt.query_map(params![session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?.max(0) as u64,
            TokenTotals::from_row(row, 2)?,
        ))
    })?;
    for row in rows {
        let (model, cache_write_1h, model_tokens) = row?;
        tokens += model_tokens;
        cost.add(&Cost::of(prices, &model, &model_tokens, cache_write_1h));
    }

    let mut time = TimeSplit::default();
    for turn in &turns {
        time += turn.split;
    }
    let started_at = first_at
        .map(clock::from_micros)
        .or_else(|| turns.first().map(|t| t.start));
    let last_activity_at = last_at
        .map(clock::from_micros)
        .or_else(|| turns.last().map(|t| t.end));

    Ok(Some(SessionDetail {
        session_id: session_id.to_owned(),
        cwd,
        git_branch,
        version,
        started_at,
        last_activity_at,
        first_prompt,
        model,
        tokens,
        cost,
        time,
        turns,
        tools,
        skills: skills::session_skill_invocations(conn, session_id)?,
        subagents: subagents::session_subagent_runs(conn, session_id)?,
    }))
}
