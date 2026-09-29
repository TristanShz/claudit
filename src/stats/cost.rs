//! API-equivalent cost: token counts priced with a [`PriceTable`].
//!
//! Every report takes the table as a parameter (the dashboard passes
//! [`PriceTable::builtin`]) and prices `api_messages` token sums at query
//! time; nothing priced is stored. Tokens are summed per model in SQL, then
//! priced and folded per group here, so a group spanning a priced and an
//! unpriced model reports the priced part as incomplete (see [`Cost`]).
//!
//! Days are UTC calendar days of the API response time.

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::{Connection, params_from_iter};
use serde::Serialize;

use super::consumption::TokenTotals;
use super::{Filter, FilterColumns};
use crate::pricing::{Cost, PriceTable};

/// Tokens and their cost for one group (a session, model, skill, …).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CostLine {
    /// The group: session id, model id, skill name or agent type (empty for
    /// [`total_cost`]).
    pub key: String,
    pub tokens: TokenTotals,
    pub cost: Cost,
}

/// One point of the daily series: a day and a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DailyUsage {
    /// UTC day.
    pub day: NaiveDate,
    pub model: String,
    pub tokens: TokenTotals,
    pub cost: Cost,
}

/// Filter columns of `api_messages m LEFT JOIN sessions s`.
const MESSAGE_COLUMNS: FilterColumns = FilterColumns {
    time_us: "m.at_us",
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some("m.model"),
};

/// Tokens and cost of every API response in the filter.
pub fn total_cost(conn: &Connection, filter: &Filter, prices: &PriceTable) -> Result<CostLine> {
    let lines = grouped(conn, filter, prices, "''", "1")?;
    Ok(lines.into_iter().next().unwrap_or_default())
}

/// Cost per session (subagents included), most expensive first.
pub fn cost_by_session(
    conn: &Connection,
    filter: &Filter,
    prices: &PriceTable,
) -> Result<Vec<CostLine>> {
    grouped(conn, filter, prices, "m.session_id", "1")
}

/// Cost per model id as reported by the API, most expensive first.
pub fn cost_by_model(
    conn: &Connection,
    filter: &Filter,
    prices: &PriceTable,
) -> Result<Vec<CostLine>> {
    grouped(conn, filter, prices, "COALESCE(m.model, '')", "1")
}

/// Cost per skill, from the responses Claude Code attributed to a skill.
pub fn cost_by_skill(
    conn: &Connection,
    filter: &Filter,
    prices: &PriceTable,
) -> Result<Vec<CostLine>> {
    grouped(conn, filter, prices, "m.skill", "m.skill IS NOT NULL")
}

/// Cost per subagent type (the responses of subagent threads).
pub fn cost_by_agent_type(
    conn: &Connection,
    filter: &Filter,
    prices: &PriceTable,
) -> Result<Vec<CostLine>> {
    grouped(
        conn,
        filter,
        prices,
        "m.agent_type",
        "m.agent_type IS NOT NULL",
    )
}

/// Tokens and cost per UTC day and model, by day then model id.
pub fn daily_series(
    conn: &Connection,
    filter: &Filter,
    prices: &PriceTable,
) -> Result<Vec<DailyUsage>> {
    let rows = per_model_sums(
        conn,
        filter,
        "date(m.at_us / 1000000, 'unixepoch')",
        "m.at_us IS NOT NULL",
    )?;
    rows.into_iter()
        .map(|(day, model, tokens, cache_write_1h)| {
            Ok(DailyUsage {
                day: NaiveDate::parse_from_str(&day, "%Y-%m-%d")?,
                cost: Cost::of(prices, &model, &tokens, cache_write_1h),
                model,
                tokens,
            })
        })
        .collect()
}

/// Groups by `key_sql`, prices each group's per-model sums and folds them.
/// Ordered by known cost (descending), then key.
fn grouped(
    conn: &Connection,
    filter: &Filter,
    prices: &PriceTable,
    key_sql: &str,
    condition: &str,
) -> Result<Vec<CostLine>> {
    let mut lines: Vec<CostLine> = Vec::new();
    for (key, model, tokens, cache_write_1h) in per_model_sums(conn, filter, key_sql, condition)? {
        if lines.last().is_none_or(|line| line.key != key) {
            lines.push(CostLine {
                key,
                ..CostLine::default()
            });
        }
        let line = lines.last_mut().expect("pushed above");
        line.tokens += tokens;
        line.cost
            .add(&Cost::of(prices, &model, &tokens, cache_write_1h));
    }
    lines.sort_by(|a, b| b.cost.known.cmp(&a.cost.known).then(a.key.cmp(&b.key)));
    Ok(lines)
}

/// `(key, model, tokens, 1-hour cache writes among tokens.cache_write)` rows
/// ordered by key then model.
fn per_model_sums(
    conn: &Connection,
    filter: &Filter,
    key_sql: &str,
    condition: &str,
) -> Result<Vec<(String, String, TokenTotals, u64)>> {
    let where_ = filter.sql(&MESSAGE_COLUMNS)?;
    let sql = format!(
        "SELECT {key_sql} AS k, COALESCE(m.model, '') AS mdl,
                COALESCE(SUM(m.cache_write_1h_tokens), 0), {sums}
         FROM api_messages m LEFT JOIN sessions s ON s.session_id = m.session_id
         WHERE {condition} AND {clause}
         GROUP BY k, mdl
         ORDER BY k, mdl",
        sums = TokenTotals::SUMS,
        clause = where_.clause,
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(where_.params), |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            TokenTotals::from_row(row, 3)?,
            row.get::<_, i64>(2)?.max(0) as u64,
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
