//! The global filter shared by every stats report.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::types::Value;

use crate::clock;

/// Restricts a report to part of the archive. `Default` means "everything".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Inclusive lower bound on event time.
    pub from: Option<DateTime<Utc>>,
    /// Exclusive upper bound on event time.
    pub to: Option<DateTime<Utc>>,
    /// Project working directory: matches this directory and everything
    /// below it.
    pub project: Option<String>,
    /// Git branch (requires session data, see `FilterColumns::branch`).
    pub branch: Option<String>,
    /// Model id (requires API message data, see `FilterColumns::model`).
    pub model: Option<String>,
}

/// Which SQL expressions a report's rows expose for each filter dimension.
/// A dimension a report cannot honour is `None`; filtering on it is then an
/// error rather than being silently ignored.
#[derive(Debug, Clone, Copy)]
pub struct FilterColumns {
    /// Event time, INTEGER µs since the epoch.
    pub time_us: &'static str,
    pub cwd: Option<&'static str>,
    pub branch: Option<&'static str>,
    pub model: Option<&'static str>,
}

/// A `WHERE` fragment (always valid, `1` when empty) and its parameters.
#[derive(Debug, Clone)]
pub struct FilterSql {
    pub clause: String,
    pub params: Vec<Value>,
}

impl Filter {
    /// Builds the `WHERE` fragment for a report exposing `columns`.
    pub fn sql(&self, columns: &FilterColumns) -> Result<FilterSql> {
        let mut conditions = Vec::new();
        let mut params = Vec::new();
        if let Some(from) = self.from {
            conditions.push(format!("{} >= ?", columns.time_us));
            params.push(Value::Integer(clock::to_micros(from)));
        }
        if let Some(to) = self.to {
            conditions.push(format!("{} < ?", columns.time_us));
            params.push(Value::Integer(clock::to_micros(to)));
        }
        if let Some(project) = &self.project {
            let Some(cwd) = columns.cwd else {
                bail!("this report cannot be filtered by project");
            };
            let dir = project.trim_end_matches('/').to_owned();
            conditions.push(format!(
                "({cwd} = ? OR substr({cwd}, 1, length(?) + 1) = ? || '/')"
            ));
            params.push(Value::Text(dir.clone()));
            params.push(Value::Text(dir.clone()));
            params.push(Value::Text(dir));
        }
        for (value, column, name) in [
            (&self.branch, columns.branch, "branch"),
            (&self.model, columns.model, "model"),
        ] {
            if let Some(value) = value {
                let Some(column) = column else {
                    bail!("this report cannot be filtered by {name} yet");
                };
                conditions.push(format!("{column} = ?"));
                params.push(Value::Text(value.clone()));
            }
        }
        let clause = if conditions.is_empty() {
            "1".to_owned()
        } else {
            conditions.join(" AND ")
        };
        Ok(FilterSql { clause, params })
    }
}
