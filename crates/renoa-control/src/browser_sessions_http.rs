use std::{sync::Arc, time::SystemTime};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    ControlError,
    browser_identity::parse_surface,
    browser_identity_http::{error_response, secure_json},
    browser_sessions::CLEAR_COOKIE,
    coordinator::CoordinatorState,
    identity_store::timestamp_millis,
};

pub(crate) fn routes() -> Router<Arc<CoordinatorState>> {
    Router::new()
        .route("/v1/identity/session", get(session))
        .route("/v1/identity/logout", post(logout))
        .route("/v1/identity/connection-ticket", post(ticket))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    principal_id: renoa_protocol::PrincipalId,
}

async fn session(State(state): State<Arc<CoordinatorState>>, headers: HeaderMap) -> Response {
    let now = SystemTime::now();
    match state.browser_sessions.authenticate(&headers, now).await {
        Ok(Some(session)) => {
            let mut response = secure_json(
                StatusCode::OK,
                &Identity {
                    principal_id: session.principal_id(),
                },
            );
            match session.cookie(now) {
                Ok(cookie) => {
                    response.headers_mut().insert(header::SET_COOKIE, cookie);
                    response
                }
                Err(error) => error_response(&error),
            }
        }
        Ok(None) => error_response(&ControlError::authentication_failed()),
        Err(error) => error_response(&error),
    }
}

async fn logout(State(state): State<Arc<CoordinatorState>>, headers: HeaderMap) -> Response {
    if !same_origin(&state, &headers) {
        return error_response(&ControlError::authentication_failed());
    }
    match state.browser_sessions.logout(&headers).await {
        Ok(()) => {
            let mut response = secure_json(StatusCode::OK, &serde_json::json!({"loggedOut":true}));
            response
                .headers_mut()
                .insert(header::SET_COOKIE, HeaderValue::from_static(CLEAR_COOKIE));
            response
        }
        Err(error) => error_response(&error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketRequest {
    surface: String,
}

async fn ticket(
    State(state): State<Arc<CoordinatorState>>,
    headers: HeaderMap,
    Json(request): Json<TicketRequest>,
) -> Response {
    if !same_origin(&state, &headers) {
        return error_response(&ControlError::authentication_failed());
    }
    let surface = match parse_surface(request.surface) {
        Ok(surface) => surface,
        Err(error) => return error_response(&error),
    };
    let now = SystemTime::now();
    let session = match state.browser_sessions.authenticate(&headers, now).await {
        Ok(Some(session)) => session,
        Ok(None) => return error_response(&ControlError::authentication_failed()),
        Err(error) => return error_response(&error),
    };
    match state
        .browser_sessions
        .connection_ticket(&session, surface, now)
        .await
    {
        Ok(Some(ticket)) => {
            let now_ms = match timestamp_millis(now) {
                Ok(now) => now,
                Err(error) => return error_response(&error),
            };
            let Some(expiry) = now_ms.checked_add(60_000) else {
                return error_response(&ControlError::store("ticket expiry overflow"));
            };
            let mut response = secure_json(
                StatusCode::OK,
                &serde_json::json!({"connectionTicket":ticket,"expiresAtMs":expiry}),
            );
            match session.cookie(now) {
                Ok(cookie) => {
                    response.headers_mut().insert(header::SET_COOKIE, cookie);
                    response
                }
                Err(error) => error_response(&error),
            }
        }
        Ok(None) => error_response(&ControlError::authentication_failed()),
        Err(error) => error_response(&error),
    }
}

fn same_origin(state: &CoordinatorState, headers: &HeaderMap) -> bool {
    state.browser_identity.as_ref().is_some_and(|identity| {
        headers
            .get(header::ORIGIN)
            .is_some_and(|origin| origin == identity.origin())
    })
}
