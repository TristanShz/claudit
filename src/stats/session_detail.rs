//! One session in detail: header, totals, turn timeline, tools, skills and
//! subagents (the session page). Not subject to filters: a session is
//! always shown whole.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};

use super::consumption::TokenTotals;
use super::cost;
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
    /// Its first typed prompt (a slash command included), else its first
    /// prompt, labelled (see [`super::prompt`]).
    pub first_prompt: Option<String>,
    /// The main thread's model (the one most of its API responses used).
    pub model: Option<String>,
    /// Tokens of every API response, subagents included.
    pub tokens: TokenTotals,
    pub cost: Cost,
    /// Every turn (user prompt) of the session, timed or not.
    pub turn_count: u64,
    /// Known only from its transcripts (see [`super::sessions`]): no time,
    /// no timeline, no tool durations.
    pub imported: bool,
    /// Its hook-timed turns' split, summed.
    pub time: TimeSplit,
    /// Main-thread turns the hooks timed, oldest first, with their
    /// positioned segments.
    pub turns: Vec<TurnTime>,
    /// Its tools, subagents' calls included (ranked as `tool_ranking`).
    pub tools: Vec<RankedCalls>,
    pub skills: Vec<SkillInvocation>,
    pub subagents: Vec<SubagentRun>,
}

/// The `sessions` row of a session (all empty when it has none).
#[derive(Default)]
struct SessionRow {
    cwd: Option<String>,
    git_branch: Option<String>,
    version: Option<String>,
    first_at_us: Option<i64>,
    last_at_us: Option<i64>,
}

impl SessionRow {
    fn load(conn: &Connection, session_id: &str) -> Result<Option<Self>> {
        Ok(conn
            .query_row(
                "SELECT cwd, git_branch, version, first_at_us, last_at_us
                 FROM sessions WHERE session_id = ?1",
                [session_id],
                |row| {
                    Ok(Self {
                        cwd: row.get(0)?,
                        git_branch: row.get(1)?,
                        version: row.get(2)?,
                        first_at_us: row.get(3)?,
                        last_at_us: row.get(4)?,
                    })
                },
            )
            .optional()?)
    }
}

/// The detail of `session_id`, `None` when nothing is known about it.
pub fn session_detail(
    conn: &Connection,
    session_id: &str,
    prices: &PriceTable,
) -> Result<Option<SessionDetail>> {
    let header = SessionRow::load(conn, session_id)?.unwrap_or_default();
    let turns = time::session_turn_times(conn, session_id)?;
    let tools = tools::session_tool_ranking(conn, session_id)?;
    let SessionRow {
        mut cwd,
        git_branch,
        version,
        first_at_us: first_at,
        last_at_us: last_at,
    } = header;
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

    let subagents = subagents::session_subagent_runs(conn, session_id)?;
    let names = subagents::display_names(&subagents);
    let first_prompt: Option<String> = conn
        .query_row(
            &format!(
                "SELECT t.prompt_text FROM turns t
                 WHERE t.session_id = ?1 AND t.prompt_text IS NOT NULL
                 ORDER BY {}, COALESCE(t.submit_at_us, t.start_at_us) LIMIT 1",
                super::prompt::INJECTED_SQL
            ),
            [session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|text| super::prompt::label(&text, &names).text);
    let model: Option<String> = conn
        .query_row(
            "SELECT model FROM api_messages
             WHERE session_id = ?1 AND agent_id IS NULL AND model IS NOT NULL
             GROUP BY model ORDER BY COUNT(*) DESC, model LIMIT 1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;

    let cost = cost::session_cost(conn, session_id, prices)?;
    let turn_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM turns WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )?;
    let imported: bool = conn.query_row(
        &format!(
            "SELECT {} FROM (SELECT ?1 AS session_id) s",
            super::sessions::IMPORTED
        ),
        [session_id],
        |row| row.get(0),
    )?;

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
        tokens: cost.tokens,
        cost: cost.cost,
        turn_count: turn_count.max(0) as u64,
        imported,
        time,
        turns,
        tools,
        skills: skills::session_skill_invocations(conn, session_id)?,
        subagents,
    }))
}
