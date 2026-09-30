//! All tools, with the Bash command and MCP server breakdowns.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;

use crate::db;
use crate::stats::tools;
use crate::web::AppState;
use crate::web::error::WebError;
use crate::web::filter_params::FilterParams;
use crate::web::format;
use crate::web::frame::Frame;
use crate::web::rows::{self, ToolRow};

#[derive(Template)]
#[template(path = "pages/tools.html")]
struct ToolsPage {
    frame: Frame,
    tools: Vec<ToolRow>,
    tools_chart_json: String,
    bash_commands: Vec<ToolRow>,
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
        Ok(ToolsPage {
            frame,
            tools_chart_json: format::script_json(&tools)?,
            tools,
            bash_commands: rows::tool_rows(tools::bash_command_ranking(&conn, &filter)?),
            mcp_servers: rows::tool_rows(tools::mcp_server_ranking(&conn, &filter)?),
        })
    })
    .await??;
    Ok(Html(page.render()?))
}
