//! All tools, with the top Bash commands (all of them on `/commands`) and
//! the MCP server breakdown.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;

use crate::db;
use crate::stats::commands::{self, CommandSort};
use crate::stats::{time, tools};
use crate::web::AppState;
use crate::web::commands_table::CommandsTable;
use crate::web::coverage::TimeNote;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;
use crate::web::frame::Frame;
use crate::web::rows::{self, ToolRow};

/// Bash commands shown here.
const TOP_COMMANDS: usize = 10;

#[derive(Template)]
#[template(path = "pages/tools.html")]
struct ToolsPage {
    frame: Frame,
    time_note: TimeNote,
    tools: Vec<ToolRow>,
    tools_chart_json: String,
    /// The Bash commands with the most runs.
    bash_commands: CommandsTable,
    /// Bash commands beyond those shown.
    more_commands: usize,
    mcp_servers: Vec<ToolRow>,
}

pub(in crate::web) async fn handler(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<FilterParams>,
) -> Result<Html<String>, WebError> {
    let page = tokio::task::spawn_blocking(move || -> anyhow::Result<ToolsPage> {
        let conn = db::open(&state.paths)?;
        let filter = filters.to_filter();
        let frame = Frame::load(&conn, &state, filters, "/tools", true)?;
        let tools = rows::tool_rows(tools::tool_ranking(&conn, &filter)?);
        let ranking = commands::command_ranking(&conn, &filter, &frame.rules, CommandSort::Calls)?;
        Ok(ToolsPage {
            frame,
            time_note: TimeNote::new(&time::time_coverage(&conn, &filter)?),
            tools_chart_json: format::script_json(&tools)?,
            tools,
            bash_commands: CommandsTable::new(&ranking, TOP_COMMANDS, None, None),
            more_commands: ranking.commands.len().saturating_sub(TOP_COMMANDS),
            mcp_servers: rows::tool_rows(tools::mcp_server_ranking(&conn, &filter)?),
        })
    })
    .await??;
    Ok(Html(page.render()?))
}
