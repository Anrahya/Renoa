use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, FromRequest, Request, State, rejection::JsonRejection},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use renoa_protocol::PrincipalId;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential};

use crate::{
    ConnectionTicket, ControlError, PasskeyBootstrapToken,
    browser_identity::{CeremonyOptions, TicketGrant, parse_surface},
    coordinator::{ControlErrorKind, CoordinatorState},
    identity_store::timestamp_millis,
};

const MAX_IDENTITY_BODY_BYTES: usize = 64 * 1024;

pub(crate) fn routes() -> Router<Arc<CoordinatorState>> {
    Router::new()
        .route(
            "/v1/identity/passkeys/registration/options",
            post(registration_options),
        )
        .route(
            "/v1/identity/passkeys/registration/verify",
            post(registration_verify),
        )
        .route(
            "/v1/identity/passkeys/authentication/options",
            post(authentication_options),
        )
        .route(
            "/v1/identity/passkeys/authentication/verify",
            post(authentication_verify),
        )
        .layer(DefaultBodyLimit::max(MAX_IDENTITY_BODY_BYTES))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationOptionsRequest {
    bootstrap_token: PasskeyBootstrapToken,
    surface: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticationOptionsRequest {
    principal_id: PrincipalId,
    surface: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyRequest<T> {
    ceremony_id: Uuid,
    credential: T,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OptionsResponse<T> {
    ceremony_id: Uuid,
    options: T,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TicketResponse {
    connection_ticket: ConnectionTicket,
    expires_at_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorResponse {
    code: &'static str,
    message: String,
}

async fn registration_options(
    State(state): State<Arc<CoordinatorState>>,
    request: Request,
) -> Response {
    let request = match json_request::<RegistrationOptionsRequest>(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let surface = match parse_surface(request.surface) {
        Ok(surface) => surface,
        Err(error) => return error_response(&error),
    };
    let Some(identity) = &state.browser_identity else {
        return unavailable_response();
    };
    match identity
        .start_registration(&state.store, request.bootstrap_token, surface)
        .await
    {
        Ok(options) => options_response(options),
        Err(error) => error_response(&error),
    }
}

async fn registration_verify(
    State(state): State<Arc<CoordinatorState>>,
    request: Request,
) -> Response {
    let request =
        match json_request::<VerifyRequest<RegisterPublicKeyCredential>>(request, &state).await {
            Ok(request) => request,
            Err(response) => return response,
        };
    let Some(identity) = &state.browser_identity else {
        return unavailable_response();
    };
    match identity
        .finish_registration(&state.store, request.ceremony_id, request.credential)
        .await
    {
        Ok(grant) => ticket_response(grant),
        Err(error) => error_response(&error),
    }
}

async fn authentication_options(
    State(state): State<Arc<CoordinatorState>>,
    request: Request,
) -> Response {
    let request = match json_request::<AuthenticationOptionsRequest>(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let surface = match parse_surface(request.surface) {
        Ok(surface) => surface,
        Err(error) => return error_response(&error),
    };
    let Some(identity) = &state.browser_identity else {
        return unavailable_response();
    };
    match identity
        .start_authentication(&state.store, request.principal_id, surface)
        .await
    {
        Ok(options) => options_response(options),
        Err(error) => error_response(&error),
    }
}

async fn authentication_verify(
    State(state): State<Arc<CoordinatorState>>,
    request: Request,
) -> Response {
    let request = match json_request::<VerifyRequest<PublicKeyCredential>>(request, &state).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(identity) = &state.browser_identity else {
        return unavailable_response();
    };
    match identity
        .finish_authentication(&state.store, request.ceremony_id, request.credential)
        .await
    {
        Ok(grant) => ticket_response(grant),
        Err(error) => error_response(&error),
    }
}

async fn json_request<T: DeserializeOwned>(
    request: Request,
    state: &Arc<CoordinatorState>,
) -> Result<T, Response> {
    Json::<T>::from_request(request, state)
        .await
        .map(|Json(value)| value)
        .map_err(|_error: JsonRejection| invalid_request_response())
}

fn options_response<T: Serialize>(options: CeremonyOptions<T>) -> Response {
    secure_json(
        StatusCode::OK,
        &OptionsResponse {
            ceremony_id: options.ceremony_id,
            options: options.options,
        },
    )
}

fn ticket_response(grant: TicketGrant) -> Response {
    let expires_at_ms = match timestamp_millis(grant.expires_at) {
        Ok(expires_at_ms) => expires_at_ms,
        Err(error) => return error_response(&error),
    };
    secure_json(
        StatusCode::OK,
        &TicketResponse {
            connection_ticket: grant.ticket,
            expires_at_ms,
        },
    )
}

fn invalid_request_response() -> Response {
    secure_json(
        StatusCode::BAD_REQUEST,
        &ErrorResponse {
            code: "invalid_request",
            message: "request is not valid Renoa identity JSON".to_owned(),
        },
    )
}

fn unavailable_response() -> Response {
    secure_json(
        StatusCode::NOT_FOUND,
        &ErrorResponse {
            code: "not_found",
            message: "identity endpoint is not available".to_owned(),
        },
    )
}

fn error_response(error: &ControlError) -> Response {
    let (status, code, message) = match error.kind() {
        ControlErrorKind::Authentication => (
            StatusCode::UNAUTHORIZED,
            "authentication_failed",
            "authentication failed".to_owned(),
        ),
        ControlErrorKind::Capacity => (
            StatusCode::TOO_MANY_REQUESTS,
            "capacity_exceeded",
            "too many passkey ceremonies are active".to_owned(),
        ),
        ControlErrorKind::Conflict => (StatusCode::CONFLICT, "conflict", error.to_string()),
        ControlErrorKind::Invalid => (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            error.to_string(),
        ),
        ControlErrorKind::NotFound => (StatusCode::NOT_FOUND, "not_found", error.to_string()),
        ControlErrorKind::Store => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "identity service failed".to_owned(),
        ),
    };
    secure_json(status, &ErrorResponse { code, message })
}

fn secure_json<T: Serialize>(status: StatusCode, value: &T) -> Response {
    let mut response = (status, Json(value)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    response
}
