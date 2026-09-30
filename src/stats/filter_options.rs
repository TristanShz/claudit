//! The values the filter bar offers: every project, branch and model in the
//! archive (not subject to filters).

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

/// Distinct filter values, each list sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FilterOptions {
    /// Session working directories.
    pub projects: Vec<String>,
    /// Session git branches.
    pub branches: Vec<String>,
    /// Models of API responses.
    pub models: Vec<String>,
}

pub fn filter_options(conn: &Connection) -> Result<FilterOptions> {
    let distinct = |sql: &str| -> Result<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    };
    Ok(FilterOptions {
        projects: distinct(
            "SELECT DISTINCT cwd FROM sessions WHERE cwd IS NOT NULL AND cwd <> '' ORDER BY cwd",
        )?,
        branches: distinct(
            "SELECT DISTINCT git_branch FROM sessions
             WHERE git_branch IS NOT NULL AND git_branch <> '' ORDER BY git_branch",
        )?,
        models: distinct(
            "SELECT DISTINCT model FROM api_messages
             WHERE model IS NOT NULL AND model <> '' ORDER BY model",
        )?,
    })
}
