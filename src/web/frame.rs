//! What every page shows around its content: the filter bar (with the
//! last ingest time and the Refresh button) and the status banners.

use std::sync::atomic::Ordering;

use rusqlite::Connection;

use super::AppState;
use super::filter_params::FilterParams;
use super::format;
use crate::stats::filter_options::{self, FilterOptions};
use crate::stats::ingest_status::{self, IngestStatus};

pub(super) struct Frame {
    pub filters: FilterParams,
    /// The filter as a query string (`""` or `?…`), appended to links.
    pub query: String,
    /// Where the filter bar submits: the current page when it is filtered,
    /// else the overview.
    pub filter_action: String,
    /// This page with its query, for the Refresh button to come back to.
    pub current: String,
    pub options: FilterOptions,
    /// e.g. `2026-03-02 09:00`, or `never`.
    pub last_ingest: String,
    /// The start-up catch-up ingest is still running.
    pub catching_up: bool,
    pub warning: Option<IngestWarning>,
    /// Anything was ever recorded (else the empty state is shown).
    pub has_data: bool,
}

/// The ingest warning banner (shown when transcript lines were skipped).
pub(super) struct IngestWarning {
    pub skipped_lines: u64,
    pub log_path: String,
}

impl Frame {
    /// `path` is the page's path; `filtered` whether the page honours the
    /// filter bar.
    pub fn load(
        conn: &Connection,
        state: &AppState,
        filters: FilterParams,
        path: &str,
        filtered: bool,
    ) -> anyhow::Result<Self> {
        let status: IngestStatus = ingest_status::ingest_status(conn)?;
        let query = filters.query();
        Ok(Self {
            filter_action: if filtered { path } else { "/" }.to_owned(),
            current: format!("{path}{query}"),
            query,
            filters,
            options: filter_options::filter_options(conn)?,
            last_ingest: status
                .last_ingest_at
                .map_or_else(|| "never".to_owned(), format::local_time),
            catching_up: state.catching_up.load(Ordering::SeqCst),
            warning: (status.skipped_transcript_lines > 0).then(|| IngestWarning {
                skipped_lines: status.skipped_transcript_lines,
                log_path: state.paths.log_file().display().to_string(),
            }),
            has_data: status.has_data,
        })
    }
}
