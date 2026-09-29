//! `claudit serve`: the local dashboard. A thin adapter: handlers call the
//! stats API and render Askama templates (`templates/`). Static assets are
//! embedded in the binary (`assets/`); nothing is fetched from the network.
//!
//! Layout: `pages/<page>.rs` holds a page's handler and template struct;
//! each page template composes one include per section
//! (`templates/sections/<section>.html`).

mod assets;
mod error;
mod filter_params;
mod format;
mod pages;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::get;

use crate::paths::Paths;

/// Port `claudit serve` listens on unless `--port` says otherwise.
pub const DEFAULT_PORT: u16 = 8421;

/// Shared state of every handler.
#[derive(Debug, Clone)]
pub struct AppState {
    pub paths: Paths,
}

/// The dashboard's routes.
pub fn router(paths: Paths) -> Router {
    Router::new()
        .route("/", get(pages::overview::handler))
        .route("/assets/{file}", get(assets::handler))
        .with_state(Arc::new(AppState { paths }))
}

/// Serves the dashboard on `127.0.0.1:<port>` until Ctrl-C.
pub async fn serve(paths: Paths, port: u16) -> Result<()> {
    // Loopback only: the archive must never be exposed on the network.
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    eprintln!("claudit dashboard: http://{}", listener.local_addr()?);
    axum::serve(listener, router(paths))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
