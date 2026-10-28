//! `/v1/host`: the signed-in owner's browser calls its Host through the node
//! that serves it. The coordinator authenticates the owner and relays the
//! call; it knows no Host operation and writes neither the request body nor
//! the answer to its files.

use std::{sync::Arc, time::SystemTime};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::Response,
    routing::{any, get},
};
use serde::Serialize;

use crate::{
    HostMethod,
    browser_identity_http::{same_origin, secure_json},
    control_log,
    coordinator::CoordinatorState,
    host_calls::{HostCallError, HostReply, HostRequest, call},
    oauth_relay_http::secure_headers,
};

/// The largest body any Host operation accepts; the node applies each
/// operation's own, smaller limit.
const MAX_BODY_BYTES: usize = 64 * 1024;

pub(crate) fn routes() -> Router<Arc<CoordinatorState>> {
    Router::new()
        .route("/v1/host/access", get(access))
        .route("/v1/host", any(relay))
        .route("/v1/host/{*path}", any(relay))
}

#[derive(Serialize)]
struct Failure<'a> {
    code: &'a str,
    message: &'a str,
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    secure_json(status, &Failure { code, message })
}

/// The public login identifier of the Host's owner; no inventory or authority.
async fn access(State(state): State<Arc<CoordinatorState>>) -> Response {
    match state.store.sole_node_owner().await {
        Ok(Some(owner)) => secure_json(
            StatusCode::OK,
            &serde_json::json!({ "owner_principal_id": owner }),
        ),
        Ok(None) => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "owner_unknown",
            "No single owner of a Host is known yet.",
        ),
        Err(error) => store_failure(&error),
    }
}

async fn relay(State(state): State<Arc<CoordinatorState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let method = match parts.method {
        Method::GET => HostMethod::Get,
        Method::POST => HostMethod::Post,
        Method::PUT => HostMethod::Put,
        _ => {
            return failure(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "Use GET, POST or PUT.",
            );
        }
    };
    // Never derive authority from Host/Forwarded headers or a caller-supplied actor.
    if method != HostMethod::Get && !same_origin(&state, &parts.headers) {
        return failure(
            StatusCode::FORBIDDEN,
            "wrong_origin",
            "Open the control panel at its configured Host address.",
        );
    }
    let now = SystemTime::now();
    let session = match state
        .browser_sessions
        .authenticate(&parts.headers, now)
        .await
    {
        Ok(Some(session)) => session,
        Ok(None) => {
            return failure(
                StatusCode::UNAUTHORIZED,
                "sign_in_required",
                "Sign in to your Host.",
            );
        }
        Err(error) => {
            control_log::event(
                "warn",
                "host_call_identity_failed",
                &serde_json::json!({ "error": error.to_string() }),
            );
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_unavailable",
                "Login storage is temporarily unavailable. Your browser will reconnect.",
            );
        }
    };
    let mut response = match read_body(&parts.headers, method, body).await {
        Ok(body) => {
            let request = HostRequest {
                method,
                path: parts.uri.path().to_owned(),
                body,
            };
            match call(&state, session.principal_id(), request).await {
                Ok(reply) => reply_response(reply),
                Err(error) => call_failure(&error),
            }
        }
        Err(response) => *response,
    };
    if let Ok(cookie) = session.cookie(now) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

async fn read_body(
    headers: &HeaderMap,
    method: HostMethod,
    body: Body,
) -> Result<Option<String>, Box<Response>> {
    if method == HostMethod::Get {
        return Ok(None);
    }
    let bytes = to_bytes(body, MAX_BODY_BYTES).await.map_err(|_| {
        Box::new(failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "The request body is too large.",
        ))
    })?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
        });
    if !json {
        return Err(Box::new(failure(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_request",
            "Send the request body as application/json.",
        )));
    }
    String::from_utf8(bytes.to_vec()).map(Some).map_err(|_| {
        Box::new(failure(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The request body must be UTF-8 JSON.",
        ))
    })
}

fn reply_response(reply: HostReply) -> Response {
    // A change is answered with a result or a refusal; an informational or
    // redirect status would send the browser somewhere the Host never named.
    let status = StatusCode::from_u16(reply.status)
        .ok()
        .filter(|status| matches!(status.as_u16(), 200..=299 | 400..=599));
    let Some(status) = status else {
        control_log::event(
            "warn",
            "host_call_invalid_status",
            &serde_json::json!({ "status": reply.status }),
        );
        return failure(
            StatusCode::BAD_GATEWAY,
            "host_invalid_reply",
            "Your Host sent an answer that could not be passed on. Retry in a moment.",
        );
    };
    let mut response = Response::new(Body::from(reply.body));
    *response.status_mut() = status;
    secure_headers(response.headers_mut());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    response
}

fn call_failure(error: &HostCallError) -> Response {
    match error {
        HostCallError::NotAnOwner => failure(
            StatusCode::FORBIDDEN,
            "wrong_owner",
            "This login does not own a Host.",
        ),
        HostCallError::Offline => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "host_offline",
            "Your Host is not connected. Retry when it returns.",
        ),
        HostCallError::Ambiguous => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "host_ambiguous",
            "More than one Host of yours is connected. Disconnect all but one.",
        ),
        HostCallError::Disconnected => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "host_disconnected",
            "Your Host disconnected before it answered. If this was a change, retry the same request to recover its outcome.",
        ),
        HostCallError::TimedOut => failure(
            StatusCode::GATEWAY_TIMEOUT,
            "host_timeout",
            "Your Host did not answer in time. If this was a change, retry the same request to recover its outcome.",
        ),
        HostCallError::Store(error) => store_failure(error),
    }
}

fn store_failure(error: &crate::ControlError) -> Response {
    control_log::event(
        "warn",
        "host_call_store_failed",
        &serde_json::json!({ "error": error.to_string() }),
    );
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "host_unavailable",
        "Your Host could not be reached. Retry in a moment.",
    )
}
