//! The Refresh button: runs a catch-up ingest, then reloads the page it was
//! pressed on.

use std::sync::Arc;

use axum::Form;
use axum::extract::State;
use axum::response::Redirect;
use serde::Deserialize;

use super::error::WebError;
use super::{AppState, run_catch_up};

#[derive(Deserialize)]
pub(super) struct RefreshForm {
    /// The page to go back to (path and query).
    #[serde(default)]
    back: String,
}

pub(super) async fn handler(
    State(state): State<Arc<AppState>>,
    Form(form): Form<RefreshForm>,
) -> Result<Redirect, WebError> {
    tokio::task::spawn_blocking(move || run_catch_up(&state.paths)).await??;
    Ok(Redirect::to(local_path(&form.back)))
}

/// `back` if it is a path on this server, else the home page (never an
/// open redirect).
fn local_path(back: &str) -> &str {
    if back.starts_with('/') && !back.starts_with("//") && !back.contains('\\') {
        back
    } else {
        "/"
    }
}
