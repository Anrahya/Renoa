//! The built Control Room, served from the coordinator's own origin so its
//! cookie, its `/v1/host` calls and its WebSocket share one HTTPS origin.

use std::path::{Path, PathBuf};

use axum::{
    Router,
    http::{HeaderValue, header},
    middleware::map_response,
    response::Response,
};
use tower_http::services::ServeDir;

use crate::ControlError;

/// Accepts a directory of built assets that has an `index.html`. The directory
/// is opened on every request, not resolved here, so a release that repoints
/// its symlink is served without a restart. Never point it at Host storage.
pub(crate) fn check(directory: &Path) -> Result<PathBuf, ControlError> {
    std::fs::File::open(directory.join("index.html")).map_err(|error| {
        ControlError::invalid(format!(
            "the Control Room directory needs an index.html: {error}"
        ))
    })?;
    Ok(directory.to_path_buf())
}

/// Serves every path no API route claims as a file, never as the app shell,
/// so an unknown path is a 404.
pub(crate) fn service(directory: PathBuf) -> Router {
    Router::new()
        .fallback_service(ServeDir::new(directory))
        .layer(map_response(shell_headers))
}

async fn shell_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
