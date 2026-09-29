//! The typed stats API: the dashboard's only data source.
//!
//! Every report is a function `fn(&Connection, &Filter) -> Result<Report>`
//! living in its own module, returning plain typed structs. Reports build
//! their `WHERE` clause with [`Filter::sql`], so every filter applies
//! uniformly.

pub mod consumption;
pub mod cost;
mod filter;
pub mod ingest_status;
pub mod sessions;
pub mod skills;
pub mod subagents;
pub mod time;
pub mod tools;

pub use filter::{Filter, FilterColumns, FilterSql};
