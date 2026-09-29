//! Consumption overview: sessions, turns and token totals.

use anyhow::Result;
use rusqlite::{Connection, Row, params_from_iter};
use serde::Serialize;

use super::{Filter, FilterColumns};

/// Token counts by class, each API response counted once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TokenTotals {
    /// Uncached input tokens.
    pub input: u64,
    pub output: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
}

impl TokenTotals {
    /// Every token, all classes together.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }

    /// The share of input-side tokens (input + cache write + cache read)
    /// served from the cache, in `0.0..=1.0`; `None` without input tokens.
    pub fn cache_read_share(&self) -> Option<f64> {
        let input_side = self.input + self.cache_write + self.cache_read;
        (input_side > 0).then(|| self.cache_read as f64 / input_side as f64)
    }

    /// SQL selecting the four sums over an `api_messages` alias `m`, in the
    /// order [`TokenTotals::from_row`] reads them.
    pub(super) const SUMS: &str = "COALESCE(SUM(m.input_tokens), 0),
         COALESCE(SUM(m.output_tokens), 0),
         COALESCE(SUM(m.cache_write_tokens), 0),
         COALESCE(SUM(m.cache_read_tokens), 0)";

    /// Reads the four sums of [`TokenTotals::SUMS`] starting at column `at`.
    pub(super) fn from_row(row: &Row, at: usize) -> rusqlite::Result<Self> {
        let get = |i: usize| row.get::<_, i64>(at + i).map(|v| v.max(0) as u64);
        Ok(Self {
            input: get(0)?,
            output: get(1)?,
            cache_write: get(2)?,
            cache_read: get(3)?,
        })
    }
}

/// Headline consumption over the filtered archive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Consumption {
    /// Sessions started in the range.
    pub sessions: u64,
    /// Turns (user prompts) started in the range.
    pub turns: u64,
    /// Tokens of API responses in the range, subagents included.
    pub tokens: TokenTotals,
}

/// Filter columns of `sessions s LEFT JOIN api_messages m`.
pub(super) const SESSION_COLUMNS: FilterColumns = FilterColumns {
    time_us: "s.first_at_us",
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some("m.model"),
};

const TURN_COLUMNS: FilterColumns = FilterColumns {
    time_us: "COALESCE(t.submit_at_us, t.start_at_us)",
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some("m.model"),
};

const MESSAGE_COLUMNS: FilterColumns = FilterColumns {
    time_us: "m.at_us",
    cwd: Some("s.cwd"),
    branch: Some("s.git_branch"),
    model: Some("m.model"),
};

/// Sessions, turns and tokens. A model filter keeps the sessions and turns
/// that used that model, and only its tokens.
pub fn consumption(conn: &Connection, filter: &Filter) -> Result<Consumption> {
    let where_ = filter.sql(&SESSION_COLUMNS)?;
    let sessions: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT s.session_id)
             FROM sessions s LEFT JOIN api_messages m ON m.session_id = s.session_id
             WHERE s.first_at_us IS NOT NULL AND {}",
            where_.clause
        ),
        params_from_iter(where_.params),
        |row| row.get(0),
    )?;

    let where_ = filter.sql(&TURN_COLUMNS)?;
    let turns: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT t.session_id || char(31) || t.prompt_id)
             FROM turns t
             LEFT JOIN sessions s ON s.session_id = t.session_id
             LEFT JOIN api_messages m
                    ON m.session_id = t.session_id AND m.prompt_id = t.prompt_id
             WHERE {}",
            where_.clause
        ),
        params_from_iter(where_.params),
        |row| row.get(0),
    )?;

    let where_ = filter.sql(&MESSAGE_COLUMNS)?;
    let tokens = conn.query_row(
        &format!(
            "SELECT {}
             FROM api_messages m LEFT JOIN sessions s ON s.session_id = m.session_id
             WHERE {}",
            TokenTotals::SUMS,
            where_.clause
        ),
        params_from_iter(where_.params),
        |row| TokenTotals::from_row(row, 0),
    )?;

    Ok(Consumption {
        sessions: sessions as u64,
        turns: turns as u64,
        tokens,
    })
}
