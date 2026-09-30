//! The typed stats API: the dashboard's only data source.
//!
//! Every report is a function `fn(&Connection, &Filter) -> Result<Report>`
//! living in its own module, returning plain typed structs. Reports build
//! their `WHERE` clause with [`Filter::sql`], so every filter applies
//! uniformly: every filtered report honours every dimension (date range,
//! project, branch, model; see `tests/filters.rs`). The few archive-wide
//! reports (`ingest_status`, `filter_options`) take no filter.

/// SQL for the model first called by the turn of a row of table alias `$t`
/// (which has `session_id`, `prompt_id` and `agent_id`), or, for a row made
/// inside a subagent, by that subagent's thread: the `model` filter column
/// of reports over tool calls, permission requests and skill invocations.
macro_rules! first_model_of_turn {
    ($t:literal) => {
        concat!(
            "(SELECT m.model FROM api_messages m WHERE m.session_id = ",
            $t,
            ".session_id AND m.prompt_id = ",
            $t,
            ".prompt_id AND m.agent_id IS ",
            $t,
            ".agent_id ORDER BY m.at_us LIMIT 1)"
        )
    };
}

pub mod consumption;
pub mod cost;
mod filter;
pub mod filter_options;
pub mod ingest_status;
pub mod session_detail;
pub mod sessions;
pub mod skills;
pub mod subagents;
pub mod time;
pub mod tools;

pub use filter::{Filter, FilterColumns, FilterSql};
