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

mod http_policy;
mod proxy;
mod ws;

#[derive(Clone)]
struct GatewayState {
    sites: Sites,
    apps: renoa_local::Apps,
    proxy: proxy::ProxyClient,
}

impl axum::extract::FromRef<GatewayState> for Sites {
    fn from_ref(state: &GatewayState) -> Self {
        state.sites.clone()
    }
}

/// Serves static published sites under `/sites/<name>/` and reverse-proxies
/// supervised live apps under `/apps/<name>/` until `shutdown` fires.
///
/// # Errors
/// Returns an [`std::io::Error`] if initializing the HTTP proxy client or
/// serving from `listener` fails.
pub async fn serve_site_gateway(
    listener: TcpListener,
    sites: Sites,
    apps: renoa_local::Apps,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let proxy = proxy::ProxyClient::new().map_err(std::io::Error::other)?;
    let state = GatewayState {
        sites,
        apps: apps.clone(),
        proxy,
    };

    let sweep_shutdown = shutdown.clone();
    let sweep_apps = apps.clone();
    let sweep_interval = (apps.idle_timeout() / 4).clamp(
        std::time::Duration::from_millis(200),
        std::time::Duration::from_secs(60),
    );
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = sweep_shutdown.cancelled() => break,
                () = tokio::time::sleep(sweep_interval) => {
                    if let Err(err) = sweep_apps.stop_idle().await {
                        crate::node_log::event(
                            "warn",
                            "live_app_idle_sweep_failed",
                            &serde_json::json!({ "error": err.to_string() }),
                        );
                    }
                }
            }
        }
    });

    let app = Router::new()
        .route("/sites/{name}", get(bare))
        .route("/sites/{name}/", get(root))
        .route("/sites/{name}/{*path}", get(file))
        .route("/apps/{name}", axum::routing::any(app_bare))
        .route("/apps/{name}/", axum::routing::any(app_root))
        .route("/apps/{name}/{*path}", axum::routing::any(app_file))
        .with_state(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

async fn app_bare(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
    uri: axum::http::Uri,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    if uri.path() != format!("/apps/{name}") {
        return http_policy::error_response(
            StatusCode::NOT_FOUND,
            "No live app is registered at this address.",
        );
    }
    match state.apps.exists(&name) {
        Ok(true) => {
            let location = match uri.query() {
                Some(q) => format!("/apps/{name}/?{q}"),
                None => format!("/apps/{name}/"),
            };
            Redirect::permanent(&location).into_response()
        }
        Ok(false) => http_policy::error_response(
            StatusCode::NOT_FOUND,
            "No live app is registered at this address.",
        ),
        Err(_) => http_policy::error_response(StatusCode::INTERNAL_SERVER_ERROR, ""),
    }
}

async fn app_root(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
    request: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    proxy::forward(&state.proxy, &state.apps, &name, request).await
}

async fn app_file(
    State(state): State<GatewayState>,
    Path((name, _path)): Path<(String, String)>,
    request: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    proxy::forward(&state.proxy, &state.apps, &name, request).await
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
