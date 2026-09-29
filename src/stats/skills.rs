//! Skills: how often each is invoked, by whom, and what it costs.
//!
//! - **Trigger**: `user` when the user typed `/<skill>` (UserPromptExpansion),
//!   `model` when Claude called the Skill tool.
//! - **Tokens**: the API messages Claude Code attributes to the skill
//!   (`attributionSkill` in the transcripts), subagents included.
//! - **Attributed time**: for each main-thread invocation, the wall time
//!   from the invocation (clamped to its turn) until the next skill
//!   invocation of the same turn, or the end of the turn. A typed skill thus
//!   owns its whole turn; a skill Claude loads mid-turn owns the rest of it.
//!   Invocations inside a subagent, or outside a known turn, get none.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::Serialize;

use super::consumption::TokenTotals;
use super::{Filter, FilterColumns};
use crate::clock;

/// Who invoked a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillTrigger {
    /// The user typed `/<skill>`.
    User,
    /// Claude called the Skill tool.
    Model,
}

/// One skill's aggregate over the filtered range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillStat {
    pub skill: String,
    pub user_invocations: u64,
    pub model_invocations: u64,
    /// See the module docs.
    pub attributed_time: Duration,
    pub tokens: TokenTotals,
}

impl SkillStat {
    pub fn invocations(&self) -> u64 {
        self.user_invocations + self.model_invocations
    }
}

/// One invocation (for a session's detail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInvocation {
    pub session_id: String,
    pub prompt_id: Option<String>,
    /// `None` on the main thread.
    pub agent_id: Option<String>,
    pub skill: String,
    pub trigger: SkillTrigger,
    /// Where a typed skill came from (`command_source`: `userSettings`,
    /// `plugin`, …); unknown for model invocations.
    pub source: Option<String>,
    pub at: DateTime<Utc>,
}

const INVOCATION_COLUMNS: FilterColumns = FilterColumns {
    time_us: "si.at_us",
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some(
        "(SELECT m.model FROM api_messages m
          WHERE m.session_id = si.session_id AND m.prompt_id = si.prompt_id
            AND m.agent_id IS si.agent_id
          ORDER BY m.at_us LIMIT 1)",
    ),
};

const MESSAGE_COLUMNS: FilterColumns = FilterColumns {
    time_us: "m.at_us",
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some("m.model"),
};

/// Skills by invocations, then tokens (descending), then name. Skills with
/// attributed tokens but no invocation in range are listed too.
pub fn skill_ranking(conn: &Connection, filter: &Filter) -> Result<Vec<SkillStat>> {
    let where_ = filter.sql(&INVOCATION_COLUMNS)?;
    let invocations = load_invocations(conn, &where_.clause, where_.params)?;

    let mut stats: BTreeMap<String, SkillStat> = BTreeMap::new();
    fn entry<'a>(stats: &'a mut BTreeMap<String, SkillStat>, skill: &str) -> &'a mut SkillStat {
        stats.entry(skill.to_owned()).or_insert_with(|| SkillStat {
            skill: skill.to_owned(),
            user_invocations: 0,
            model_invocations: 0,
            attributed_time: Duration::zero(),
            tokens: TokenTotals::default(),
        })
    }
    let times = attributed_times(conn, &invocations)?;
    for (invocation, time) in invocations.iter().zip(times) {
        let stat = entry(&mut stats, &invocation.skill);
        match invocation.trigger {
            SkillTrigger::User => stat.user_invocations += 1,
            SkillTrigger::Model => stat.model_invocations += 1,
        }
        stat.attributed_time += time;
    }

    let where_ = filter.sql(&MESSAGE_COLUMNS)?;
    let sql = format!(
        "SELECT m.skill, {}
         FROM api_messages m LEFT JOIN sessions s ON s.session_id = m.session_id
         WHERE m.skill IS NOT NULL AND m.skill <> '' AND {}
         GROUP BY m.skill",
        TokenTotals::SUMS,
        where_.clause
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(where_.params), |row| {
        Ok((row.get::<_, String>(0)?, TokenTotals::from_row(row, 1)?))
    })?;
    for row in rows {
        let (skill, tokens) = row?;
        entry(&mut stats, &skill).tokens = tokens;
    }

    let mut stats: Vec<SkillStat> = stats.into_values().collect();
    stats.sort_by(|a, b| {
        b.invocations()
            .cmp(&a.invocations())
            .then(b.tokens.total().cmp(&a.tokens.total()))
            .then_with(|| a.skill.cmp(&b.skill))
    });
    Ok(stats)
}

/// Every skill invocation of one session, in time order.
pub fn session_skill_invocations(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<SkillInvocation>> {
    load_invocations(
        conn,
        "si.session_id = ?",
        vec![Value::Text(session_id.to_owned())],
    )
}

fn load_invocations(
    conn: &Connection,
    clause: &str,
    params: Vec<Value>,
) -> Result<Vec<SkillInvocation>> {
    let sql = format!(
        "SELECT si.session_id, si.prompt_id, si.agent_id, si.skill, si.trigger, si.source, si.at_us
         FROM skill_invocations si LEFT JOIN sessions s ON s.session_id = si.session_id
         WHERE {clause}
         ORDER BY si.at_us, si.session_id, si.invocation_id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        Ok(SkillInvocation {
            session_id: row.get(0)?,
            prompt_id: row.get(1)?,
            agent_id: row.get(2)?,
            skill: row.get(3)?,
            trigger: if row.get::<_, String>(4)? == "user" {
                SkillTrigger::User
            } else {
                SkillTrigger::Model
            },
            source: row.get(5)?,
            at: clock::from_micros(row.get(6)?),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Each invocation's attributed time (see the module docs), in order.
fn attributed_times(conn: &Connection, invocations: &[SkillInvocation]) -> Result<Vec<Duration>> {
    let mut window_stmt = conn.prepare(
        "SELECT COALESCE(submit_at_us, start_at_us), COALESCE(stop_at_us, end_at_us)
         FROM turns WHERE session_id = ?1 AND prompt_id = ?2",
    )?;
    let mut next_stmt = conn.prepare(
        "SELECT MIN(at_us) FROM skill_invocations
         WHERE session_id = ?1 AND prompt_id = ?2 AND agent_id IS NULL AND at_us > ?3",
    )?;
    let mut times = Vec::with_capacity(invocations.len());
    for invocation in invocations {
        let (Some(prompt_id), None) = (&invocation.prompt_id, &invocation.agent_id) else {
            times.push(Duration::zero());
            continue;
        };
        let window: Option<(Option<i64>, Option<i64>)> = window_stmt
            .query_row(params![invocation.session_id, prompt_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        let Some((Some(start), Some(end))) = window else {
            times.push(Duration::zero());
            continue;
        };
        let at = clock::to_micros(invocation.at);
        let next: Option<i64> = next_stmt
            .query_row(params![invocation.session_id, prompt_id, at], |row| {
                row.get(0)
            })?;
        let from = at.clamp(start, end.max(start));
        let to = next.unwrap_or(end).clamp(from, end.max(from));
        times.push(Duration::microseconds(to - from));
    }
    Ok(times)
}
