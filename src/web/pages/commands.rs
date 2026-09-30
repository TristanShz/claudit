//! Every Bash command (by command key), sortable by runs, total time,
//! median, p95 or failures.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;

use crate::activities::WAITING;
use crate::db;
use crate::stats::commands::{self, CommandSort};
use crate::stats::time;
use crate::web::AppState;
use crate::web::commands_table::{CommandsTable, SortParam, page_href};
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
        let show_polling = sort.polling == "show";
        let sort_name = sort.sort.clone();
        let sort = sort.sort_or(CommandSort::Total);
        let mut ranking = commands::command_ranking(&conn, &filter, &frame.rules, sort)?;
        // Waiting & polling is hidden unless asked for: its time is spent
        // waiting on something else, and would top the slowest commands.
        let hidden = if show_polling {
            Default::default()
        } else {
            ranking.hide_activity(WAITING)
        };
        let polling: &[(&str, &str)] = if show_polling {
            &[("polling", "show")]
        } else {
            &[]
        };
        let link = |name: &str| {
            let mut params = vec![("sort", name)];
            params.extend_from_slice(polling);
            page_href("/commands", &frame.query, &params, "")
        };
        let sorted: &[(&str, &str)] = if sort_name.is_empty() {
            &[]
        } else {
            &[("sort", sort_name.as_str())]
        };
        let mut table = CommandsTable::new(&ranking, usize::MAX, Some(sort), Some(&link));
        table = if show_polling {
            table.with_hide_link(page_href("/commands", &frame.query, sorted, ""))
        } else {
            let mut params = sorted.to_vec();
            params.push(("polling", "show"));
            table.with_hidden(hidden, page_href("/commands", &frame.query, &params, ""))
        };
        Ok(CommandsPage {
            table,
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
