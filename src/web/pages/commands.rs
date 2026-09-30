//! Every Bash command (by command key), sortable by runs, total time,
//! median, p95 or failures.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;

use crate::db;
use crate::stats::commands::{self, CommandSort};
use crate::stats::time;
use crate::web::AppState;
use crate::web::commands_table::{CommandsTable, SortParam, sort_href};
use crate::web::coverage::TimeNote;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;
use crate::web::frame::Frame;

#[derive(Template)]
#[template(path = "pages/commands.html")]
struct CommandsPage {
    frame: Frame,
    time_note: TimeNote,
    table: CommandsTable,
    /// Summed hook-timed Bash time, formatted (`–` without any).
    total: String,
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<FilterParams>,
    Query(sort): Query<SortParam>,
) -> Result<Html<String>, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<CommandsPage> {
        let conn = db::open(&state.paths)?;
        let filter = filters.to_filter();
        let frame = Frame::load(&conn, &state, filters, "/commands", true)?;
        let sort = sort.sort_or(CommandSort::Total);
        let ranking = commands::command_ranking(&conn, &filter, &frame.rules, sort)?;
        let link = |name: &str| sort_href("/commands", &frame.query, name, "");
        Ok(CommandsPage {
            table: CommandsTable::new(&ranking, usize::MAX, Some(sort), Some(&link)),
            total: if ranking.timed_calls == 0 {
                "–".to_owned()
            } else {
                format::duration_ms(ranking.total_duration_ms)
            },
            time_note: TimeNote::new(&time::time_coverage(&conn, &filter)?),
            frame,
        })
    })
    .await??;
    Ok(Html(page.render()?))
}
