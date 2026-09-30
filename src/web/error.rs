//! Turning handler failures into a 500 page (details go to stderr).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub(super) struct WebError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for WebError {
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        eprintln!("claudit serve: {:#}", self.0);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Error: {:#}", self.0),
        )
            .into_response()
    }
}
