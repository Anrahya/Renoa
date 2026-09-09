//! HTTP presentation of existing Host operations. No agent execution lives here.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path as RequestPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use renoa_local::{HostObserver, HostReviewControl, HostRoutineControl};
use renoa_protocol::PrincipalId;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod identity;
mod reviews;
mod routines;

fn origin_failure(state: &ManagementState, headers: &HeaderMap) -> Option<Response> {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    (origins
        .next()
        .is_none_or(|value| value != state.origin.as_str())
        || origins.next().is_some())
    .then(|| {
        failure(
            StatusCode::FORBIDDEN,
            "wrong_origin",
            "Open the control panel at its configured Host address.",
        )
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ManagementError {
    #[error(transparent)]
    Host(#[from] renoa_local::LocalHostError),
    #[error(transparent)]
    Identity(#[from] reqwest::Error),
    #[error("identity service is unavailable")]
    IdentityUnavailable,
    #[error("the identity service must be reached through loopback")]
    PublicIdentity,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("configured Host identity does not match the existing data directory")]
    HostMismatch,
    #[error("the plaintext management listener must be loopback-only")]
    PublicListener,
    #[error(
        "public_origin must be an HTTPS origin (or HTTP localhost), without a path or credentials"
    )]
    InvalidOrigin,
}

#[derive(Clone)]
pub struct ManagementApi {
    state: Arc<ManagementState>,
    assets: Option<PathBuf>,
}

struct ManagementState {
    observer: HostObserver,
    identity: identity::IdentityClient,
    owner: PrincipalId,
    routines: HostRoutineControl,
    reviews: HostReviewControl,
    origin: String,
}

impl ManagementApi {
    /// Binds one existing Host to an explicitly configured authenticated owner.
    /// Does not initialize a Host, discover providers, or load credentials.
    /// # Errors
    /// Returns unavailable storage, a mismatched Host identity, or invalid origin
    /// or identity-service configuration.
    pub fn open(
        root: &Path,
        host_id: Uuid,
        identity_address: SocketAddr,
        owner: PrincipalId,
        public_origin: &str,
    ) -> Result<Self, ManagementError> {
        let observer = HostObserver::open(root)?;
        if observer.host_id() != host_id {
            return Err(ManagementError::HostMismatch);
        }
        let origin = routines::validate_origin(public_origin)?;
        Ok(Self {
            assets: None,
            state: Arc::new(ManagementState {
                observer,
                identity: identity::IdentityClient::new(identity_address)?,
                owner,
                routines: HostRoutineControl::open(root, host_id, owner.as_uuid())?,
                reviews: HostReviewControl::open(root, host_id, owner.as_uuid())?,
                origin,
            }),
        })
    }

    /// Serves a built control panel from a dedicated public asset directory.
    /// # Errors
    /// Returns a missing directory or index file. Never point this at Host storage.
    pub fn with_assets(mut self, directory: &Path) -> Result<Self, ManagementError> {
        let directory = std::fs::canonicalize(directory)?;
        std::fs::File::open(directory.join("index.html"))?;
        self.assets = Some(directory);
        Ok(self)
    }

    /// Serves behind the deployment's HTTPS proxy, without owning an execution worker.
    /// # Errors
    /// Returns listener or server failures. Public plaintext listeners are rejected.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: CancellationToken,
    ) -> Result<(), ManagementError> {
        if !listener.local_addr()?.ip().is_loopback() {
            return Err(ManagementError::PublicListener);
        }
        let mut app = Router::new()
            .route("/v1/host/access", get(access))
            .route("/v1/host", get(observe))
            .route("/v1/host/reviews/{request_id}", get(review_detail))
            .route(
                "/v1/host/repositories/{repository_id}/policy",
                axum::routing::post(reviews::update_policy),
            )
            .route(
                "/v1/host/routines/{routine_id}/enabled",
                axum::routing::post(routines::set_enabled),
            )
            .layer(DefaultBodyLimit::max(4096))
            .route(
                "/v1/{*path}",
                axum::routing::any(|| async { StatusCode::NOT_FOUND }),
            )
            .with_state(self.state);
        if let Some(directory) = self.assets {
            app = app.fallback_service(tower_http::services::ServeDir::new(directory));
        }
        let app = app.layer(axum::middleware::map_response(|response: Response| async {
            secure(response)
        }));
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await?;
        Ok(())
    }
}

#[derive(Serialize)]
struct Access {
    owner_principal_id: PrincipalId,
}

async fn access(State(state): State<Arc<ManagementState>>) -> Response {
    // Public login identifier only: no Host inventory or authority is conveyed.
    secure(
        (
            StatusCode::OK,
            Json(Access {
                owner_principal_id: state.owner,
            }),
        )
            .into_response(),
    )
}

async fn observe(State(state): State<Arc<ManagementState>>, headers: HeaderMap) -> Response {
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match state.observer.snapshot().await {
        Ok(snapshot) => {
            let mut response = secure(Json(snapshot).into_response());
            if let Some(cookie) = session.renewal {
                response.headers_mut().insert(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(error) => {
            eprintln!("Host management observation failed: {error}");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "host_unavailable",
                "Host records are temporarily unavailable. Showing the last received state.",
            )
        }
    }
}

async fn review_detail(
    State(state): State<Arc<ManagementState>>,
    headers: HeaderMap,
    RequestPath(request): RequestPath<Uuid>,
) -> Response {
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    match state.observer.review_detail(request).await {
        Ok(Some(detail)) => {
            let mut response = secure(Json(detail).into_response());
            if let Some(cookie) = session.renewal {
                response.headers_mut().insert(header::SET_COOKIE, cookie);
            }
            response
        }
        Ok(None) => failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "Review not found in this Host.",
        ),
        Err(error) => {
            eprintln!("Host management review detail failed: {error}");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "host_unavailable",
                "Review details are temporarily unavailable.",
            )
        }
    }
}

async fn authorize(
    state: &ManagementState,
    headers: &HeaderMap,
) -> Result<identity::Authenticated, Response> {
    match state.identity.authenticate(headers).await {
        Ok(Some(session)) if session.principal == state.owner => Ok(session),
        Ok(Some(_)) => Err(failure(
            StatusCode::FORBIDDEN,
            "wrong_owner",
            "This login does not own the configured Host.",
        )),
        Ok(None) => Err(failure(
            StatusCode::UNAUTHORIZED,
            "sign_in_required",
            "Sign in to your Host.",
        )),
        Err(error) => {
            eprintln!("Host management identity storage failed: {error}");
            Err(failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_unavailable",
                "Login storage is temporarily unavailable. Your browser will reconnect.",
            ))
        }
    }
}

#[derive(Serialize)]
struct Failure<'a> {
    code: &'a str,
    message: &'a str,
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    secure((status, Json(Failure { code, message })).into_response())
}

fn secure(mut response: Response) -> Response {
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
