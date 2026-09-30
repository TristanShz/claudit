//! `claudit serve`: the local dashboard. A thin adapter: handlers call the
//! stats API and render Askama templates (`templates/`). Static assets are
//! embedded in the binary (`assets/`); nothing is fetched from the network.
//!
//! Layout: `pages/<page>.rs` holds a page's handler and template struct;
//! each page template composes one include per section
//! (`templates/sections/<section>.html`). [`frame::Frame`] carries what
//! every page shows around its content: the filter bar and the banners.

mod assets;
mod commands_table;
mod coverage;
mod error;
mod filter_params;
mod format;
mod frame;
mod pages;
mod refresh;
mod rows;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::{get, post};

use crate::clock::SystemClock;
use crate::ingest::{self, IngestOutcome};
use crate::logfile;
use crate::paths::Paths;

/// Port `claudit serve` listens on unless `--port` says otherwise.
pub const DEFAULT_PORT: u16 = 8421;

/// Shared state of every handler.
#[derive(Debug, Clone)]
pub struct AppState {
    pub paths: Paths,
    /// True while the start-up catch-up ingest is still running.
    pub catching_up: Arc<AtomicBool>,
}

/// The dashboard's routes, over an archive that is already caught up.
pub fn router(paths: Paths) -> Router {
    app(AppState {
        paths,
        catching_up: Arc::new(AtomicBool::new(false)),
    })
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(pages::overview::handler))
        .route("/tools", get(pages::tools::handler))
        .route("/commands", get(pages::commands::handler))
        .route("/activities", get(pages::activities::handler))
        .route("/skills", get(pages::skills::handler))
        .route("/subagents", get(pages::subagents::handler))
        .route("/models", get(pages::models::handler))
        .route("/sessions", get(pages::sessions::handler))
        .route("/sessions/{id}", get(pages::session::handler))
        .route(
            "/sessions/{id}/turns/{prompt_id}",
            get(pages::turn_trace::handler),
        )
        .route("/refresh", post(refresh::handler))
        .route("/assets/{file}", get(assets::handler))
        .with_state(Arc::new(state))
}

/// Serves the dashboard on `127.0.0.1:<port>` until Ctrl-C. The catch-up
/// ingest runs in the background once the port is bound; pages show a
/// banner until it is done (the dashboard never polls: reload to see).
pub async fn serve(paths: Paths, port: u16) -> Result<()> {
    // Loopback only: the archive must never be exposed on the network.
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    eprintln!("claudit dashboard: http://{}", listener.local_addr()?);

    let catching_up = Arc::new(AtomicBool::new(true));
    let flag = catching_up.clone();
    let ingest_paths = paths.clone();
    tokio::task::spawn_blocking(move || {
        eprintln!("claudit: catching up on pending ingestion in the background…");
        match run_catch_up(&ingest_paths) {
            Ok(message) => eprintln!("claudit: {message}"),
            Err(err) => eprintln!("claudit: catch-up failed: {err:#}"),
        }
        flag.store(false, Ordering::SeqCst);
    });

    axum::serve(listener, app(AppState { paths, catching_up }))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// Runs the locked catch-up ingest; failures also go to the error log.
fn run_catch_up(paths: &Paths) -> Result<String> {
    match ingest::catch_up(paths, &SystemClock) {
        Ok(IngestOutcome::Ran(report)) => Ok(format!(
            "caught up: {}{} events, {} transcript lines",
            if report.rebuilt {
                "rebuilt the archive once after the upgrade, "
            } else {
                ""
            },
            report.events,
            report.transcript_lines
        )),
        Ok(IngestOutcome::AlreadyRunning) => {
            Ok("another ingest is running; it will pick up pending input".to_owned())
        }
        Err(err) => {
            logfile::error(paths, "ingest", format!("{err:#}"));
            Err(err)
        }
    }
}
