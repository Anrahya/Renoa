//! The two routes a computer signs in through an approving app with: asking
//! for a code, and collecting the credential once the code was approved.

use std::{sync::Arc, time::SystemTime};

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::StatusCode,
    response::Response,
    routing::post,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    DeviceCredentials,
    browser_identity::parse_surface,
    browser_identity_http::{MAX_IDENTITY_BODY_BYTES, error_response, json_request, secure_json},
    coordinator::CoordinatorState,
    device_key::DeviceRegistration,
    device_requests::{Collected, NewRequest},
};

pub(crate) fn routes() -> Router<Arc<CoordinatorState>> {
    Router::new()
        .route("/v1/identity/device-requests", post(open))
        .route("/v1/identity/device-requests/collect", post(collect))
        .layer(DefaultBodyLimit::max(MAX_IDENTITY_BODY_BYTES))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceBody {
    public_key: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenBody {
    request_id: Uuid,
    surface: String,
    device: DeviceBody,
    device_proof: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectBody {
    request_id: Uuid,
    device_proof: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CodeResponse {
    code: String,
    expires_at_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WaitingResponse {
    expires_at_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignedInResponse {
    credentials: DeviceCredentials,
}

/// Every check runs before the request is stored: a malformed surface, key
/// or label, or a proof the key did not make, leaves nothing behind.
async fn open(State(state): State<Arc<CoordinatorState>>, request: Request) -> Response {
    let body = match json_request::<OpenBody>(request, &state).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let parsed = parse_surface(body.surface).and_then(|surface| {
        let registration = DeviceRegistration::parse(&body.device.public_key, &body.device.label)?;
        NewRequest::proven(body.request_id, surface, registration, &body.device_proof)
    });
    let request = match parsed {
        Ok(request) => request,
        Err(error) => return error_response(&error),
    };
    match state
        .store
        .open_device_request(request, SystemTime::now())
        .await
    {
        Ok(opened) => secure_json(
            StatusCode::OK,
            &CodeResponse {
                code: opened.code,
                expires_at_ms: opened.expires_at_ms,
            },
        ),
        Err(error) => error_response(&error),
    }
}

/// `202` while no app has approved the request, `200` with the credential
/// once one has. A device signed in again has its old credential replaced,
/// so its live connections end before the new credential is returned.
async fn collect(State(state): State<Arc<CoordinatorState>>, request: Request) -> Response {
    let body = match json_request::<CollectBody>(request, &state).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
    match state
        .store
        .collect_device_request(body.request_id, body.device_proof, SystemTime::now())
        .await
    {
        Ok(Collected::Waiting { expires_at_ms }) => {
            secure_json(StatusCode::ACCEPTED, &WaitingResponse { expires_at_ms })
        }
        Ok(Collected::SignedIn(principal_id, signed_in)) => {
            crate::browser_identity::log_device_sign_in(principal_id, &signed_in);
            if signed_in.adopted {
                crate::device_admin::end_device_sessions(&state, signed_in.credentials.device_id)
                    .await;
            }
            secure_json(
                StatusCode::OK,
                &SignedInResponse {
                    credentials: signed_in.credentials,
                },
            )
        }
        Err(error) => error_response(&error),
    }
}
