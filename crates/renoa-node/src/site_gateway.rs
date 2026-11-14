//! The site gateway: published static sites served to the owner's devices.
//!
//! It listens on a loopback address only. The owner puts one Tailscale Serve
//! entry in front of it, so it is reached from the owner's tailnet and from
//! nowhere else, and it asks no one to sign in. Every site is served under
//! `/sites/<name>/` on the gateway's one origin, straight from its snapshot on
//! disk, with byte ranges so clips can seek. Nothing on the gateway's origin
//! carries a Renoa credential, so a site's scripts can reach nothing of
//! Renoa's.

use axum::{
    Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Redirect, Response},
    routing::get,
};
use renoa_local::{SiteEntry, SiteError, Sites};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeFile;

use crate::node_log;

/// Serves published sites on `listener` until `shutdown`.
///
/// # Errors
/// Returns the listener's failure.
pub async fn serve_site_gateway(
    listener: TcpListener,
    sites: Sites,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let app = Router::new()
        .route("/sites/{name}", get(bare))
        .route("/sites/{name}/", get(root))
        .route("/sites/{name}/{*path}", get(file))
        .with_state(sites);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

/// A site's bare name goes to its root, so relative links resolve inside it.
async fn bare(State(sites): State<Sites>, Path(name): Path<String>) -> Response {
    match sites.entry(&name, "").await {
        Ok(_) => Redirect::permanent(&format!("/sites/{name}/")).into_response(),
        Err(error) => failure(&error),
    }
}

async fn root(State(sites): State<Sites>, Path(name): Path<String>, request: Request) -> Response {
    serve(&sites, &name, "", request).await
}

async fn file(
    State(sites): State<Sites>,
    Path((name, path)): Path<(String, String)>,
    request: Request,
) -> Response {
    serve(&sites, &name, &path, request).await
}

async fn serve(sites: &Sites, name: &str, path: &str, request: Request) -> Response {
    let mut response = match sites.entry(name, path).await {
        Ok(SiteEntry::File(file)) => match ServeFile::new(file).try_call(request).await {
            Ok(served) => served.map(Body::new),
            Err(error) => {
                node_log::event(
                    "warn",
                    "site_gateway_read_failed",
                    &serde_json::json!({ "site": name, "error": error.to_string() }),
                );
                return plain(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE);
            }
        },
        Ok(SiteEntry::Listing(html)) => {
            let mut listing = Response::new(Body::from(html));
            listing.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            listing
        }
        Err(error) => return failure(&error),
    };
    let headers = response.headers_mut();
    // A republished site replaces its files at the same paths.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

const UNAVAILABLE: &str = "This site could not be read just now. Try again in a moment.";

fn failure(error: &SiteError) -> Response {
    if matches!(error, SiteError::NotFound) {
        return plain(
            StatusCode::NOT_FOUND,
            "Nothing is published at this address.",
        );
    }
    node_log::event(
        "warn",
        "site_gateway_failed",
        &serde_json::json!({ "error": error.to_string() }),
    );
    plain(StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE)
}

fn plain(status: StatusCode, message: &'static str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        message,
    )
        .into_response()
}
