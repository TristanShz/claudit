//! The typed stats API: the dashboard's only data source.
//!
//! Every report is a function `fn(&Connection, &Filter) -> Result<Report>`
//! living in its own module, returning plain typed structs. Reports build
//! their `WHERE` clause with [`Filter::sql`], so every filter applies
//! uniformly: every filtered report honours every dimension (date range,
//! project, branch, model; see `tests/filters.rs`). The few archive-wide
//! reports (`ingest_status`, `filter_options`) take no filter.

pub mod consumption;
pub mod cost;
mod filter;
pub mod filter_options;
pub mod ingest_status;
pub mod sessions;
pub mod skills;
pub mod subagents;
pub mod time;
pub mod tools;

pub use filter::{Filter, FilterColumns, FilterSql};
