//! Models: what each model was used for and what it cost.
//!
//! Per model id as the API reports it: the sessions that used it, its API
//! responses, their tokens and API-equivalent cost (priced with a
//! [`PriceTable`] at query time, as in [`super::cost`]), split between the
//! main thread and subagent threads.
//!
//! Filters apply to API responses, as in [`super::cost`]: the date range to
//! the response time, project and branch to its session, and model to the
//! response's model (so a model filter keeps that model only).

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::{Connection, params_from_iter};
use serde::Serialize;

use super::consumption::TokenTotals;
use super::{Filter, FilterColumns};
use crate::pricing::{Cost, PriceTable};

/// The responses of one model on one kind of thread.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ThreadUsage {
    pub api_messages: u64,
    pub tokens: TokenTotals,
    pub cost: Cost,
}

impl ThreadUsage {
    fn add(&mut self, other: &ThreadUsage) {
        self.api_messages += other.api_messages;
        self.tokens += other.tokens;
        self.cost.add(&other.cost);
    }
}

/// One model's usage over the filter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModelUsage {
    /// Model id as the API reports it (empty when a response had none).
    pub model: String,
    /// Sessions with at least one response of this model.
    pub sessions: u64,
    /// API responses, both threads.
    pub api_messages: u64,
    pub tokens: TokenTotals,
    /// Cost of `tokens`; unknown when the price table lacks the model.
    pub cost: Cost,
    /// Main-thread responses.
    pub main: ThreadUsage,
    /// Responses of subagent threads.
    pub subagents: ThreadUsage,
}

/// Every model's usage, plus the totals shares are taken of.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModelsReport {
    /// Most expensive (known cost) first, then most tokens, then model id.
    pub models: Vec<ModelUsage>,
    pub total_tokens: TokenTotals,
    pub total_cost: Cost,
}

impl ModelsReport {
    /// `model`'s share of every token in the report, in `[0, 1]`.
    pub fn token_share(&self, model: &ModelUsage) -> f64 {
        let total = self.total_tokens.total();
        if total == 0 {
            0.0
        } else {
            model.tokens.total() as f64 / total as f64
        }
    }

    /// `model`'s share of the priced cost, in `[0, 1]`; `None` when its own
    /// cost is unknown. When other models are unpriced this is a share of
    /// the known part only.
    pub fn cost_share(&self, model: &ModelUsage) -> Option<f64> {
        let own = model.cost.total()?;
        let total = self.total_cost.known.picos();
        Some(if total == 0 {
            0.0
        } else {
            own.picos() as f64 / total as f64
        })
    }
}

/// Filter columns of `api_messages m LEFT JOIN sessions s`.
const MESSAGE_COLUMNS: FilterColumns = FilterColumns {
    time_us: "m.at_us",
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some("m.model"),
};

/// Usage and cost per model over the filtered API responses.
pub fn model_usage(
    conn: &Connection,
    filter: &Filter,
    prices: &PriceTable,
) -> Result<ModelsReport> {
    let where_ = filter.sql(&MESSAGE_COLUMNS)?;
    let sql = format!(
        "SELECT COALESCE(m.model, '') AS mdl, m.agent_id IS NOT NULL AS sub, COUNT(*),
                COALESCE(SUM(m.cache_write_1h_tokens), 0), {sums}
         FROM api_messages m LEFT JOIN sessions s ON s.session_id = m.session_id
         WHERE {clause}
         GROUP BY mdl, sub",
        sums = TokenTotals::SUMS,
        clause = where_.clause,
    );
    let mut by_model: BTreeMap<String, ModelUsage> = BTreeMap::new();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(where_.params.iter()))?;
    while let Some(row) = rows.next()? {
        let model: String = row.get(0)?;
        let subagent: bool = row.get(1)?;
        let tokens = TokenTotals::from_row(row, 4)?;
        let thread = ThreadUsage {
            api_messages: row.get::<_, i64>(2)?.max(0) as u64,
            cost: Cost::of(prices, &model, &tokens, row.get::<_, i64>(3)?.max(0) as u64),
            tokens,
        };
        let usage = by_model.entry(model.clone()).or_insert_with(|| ModelUsage {
            model,
            ..ModelUsage::default()
        });
        if subagent {
            usage.subagents.add(&thread);
        } else {
            usage.main.add(&thread);
        }
    }

    let sessions_sql = format!(
        "SELECT COALESCE(m.model, '') AS mdl, COUNT(DISTINCT m.session_id)
         FROM api_messages m LEFT JOIN sessions s ON s.session_id = m.session_id
         WHERE {}
         GROUP BY mdl",
        where_.clause
    );
    let mut stmt = conn.prepare(&sessions_sql)?;
    let mut rows = stmt.query(params_from_iter(where_.params.iter()))?;
    while let Some(row) = rows.next()? {
        if let Some(usage) = by_model.get_mut(&row.get::<_, String>(0)?) {
            usage.sessions = row.get::<_, i64>(1)?.max(0) as u64;
        }
    }

    let mut report = ModelsReport::default();
    for mut usage in by_model.into_values() {
        let mut both = usage.main.clone();
        both.add(&usage.subagents);
        usage.api_messages = both.api_messages;
        usage.tokens = both.tokens;
        usage.cost = both.cost;
        report.total_tokens += usage.tokens;
        report.total_cost.add(&usage.cost);
        report.models.push(usage);
    }
    report.models.sort_by(|a, b| {
        b.cost
            .known
            .cmp(&a.cost.known)
            .then(b.tokens.total().cmp(&a.tokens.total()))
            .then_with(|| a.model.cmp(&b.model))
    });
    Ok(report)
}
