//! Static assets compiled into the binary.

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

/// `(file name, content type, bytes)` of every embedded asset.
const ASSETS: &[(&str, &str, &[u8])] = &[
    (
        "htmx.min.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../../assets/htmx.min.js"),
    ),
    (
        "echarts.min.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../../assets/echarts.min.js"),
    ),
    (
        "claudit.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../../assets/claudit.js"),
    ),
    (
        "claudit.css",
        "text/css; charset=utf-8",
        include_bytes!("../../assets/claudit.css"),
    ),
];

pub(super) async fn handler(Path(file): Path<String>) -> Response {
    match ASSETS.iter().find(|(name, _, _)| *name == file) {
        Some((_, content_type, bytes)) => (
            [
                (header::CONTENT_TYPE, *content_type),
                (header::CACHE_CONTROL, "public, max-age=3600"),
            ],
            *bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
