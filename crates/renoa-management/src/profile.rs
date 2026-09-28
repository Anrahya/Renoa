//! The signed-in owner's own `USER.md`, which agent creation shows so the owner
//! can decide whether a new agent reads it.

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response},
};
use renoa_local::ProfileEditError;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::{
    ManagementState,
    agents::{renew, unavailable},
    authorize, failure, origin_failure,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProfileEdit {
    expected_revision: String,
    content: String,
}

pub(super) async fn read(
    State(state): State<Arc<ManagementState>>,
    headers: HeaderMap,
) -> Response {
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return *response,
    };
    let Some(host) = &state.agents else {
        return unavailable();
    };
    if host.host_id().await.ok() != Some(state.observer.host_id()) {
        return unavailable();
    }
    let mut response = match host.user_profile(session.principal.as_uuid()).await {
        Ok(profile) => Json(profile).into_response(),
        Err(error) => {
            eprintln!("Host management profile read failed: {error}");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "profile_unavailable",
                "Your profile could not be read. Retry in a moment.",
            )
        }
    };
    renew(&mut response, session.renewal);
    response
}

pub(super) async fn replace(
    State(state): State<Arc<ManagementState>>,
    headers: HeaderMap,
    request: Result<Json<ProfileEdit>, JsonRejection>,
) -> Response {
    if let Some(response) = origin_failure(&state, &headers) {
        return response;
    }
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return *response,
    };
    let Some(host) = &state.agents else {
        return unavailable();
    };
    if host.host_id().await.ok() != Some(state.observer.host_id()) {
        return unavailable();
    }
    let request = match request {
        Ok(Json(request)) => request,
        Err(error) => {
            return failure(
                error.status(),
                "invalid_request",
                "Send the profile's expected_revision and its complete new content as JSON.",
            );
        }
    };
    let mut response = match host
        .replace_user_profile(
            session.principal.as_uuid(),
            &request.expected_revision,
            request.content,
            &CancellationToken::new(),
        )
        .await
    {
        Ok(profile) => Json(profile).into_response(),
        Err(ProfileEditError::Stale) => failure(
            StatusCode::CONFLICT,
            "revision_conflict",
            "Your profile changed since you opened it. Reload it before saving.",
        ),
        Err(ProfileEditError::Invalid(reason)) => {
            failure(StatusCode::UNPROCESSABLE_ENTITY, "invalid_profile", &reason)
        }
        Err(ProfileEditError::Unavailable(reason)) => {
            eprintln!("Host management profile edit failed: {reason}");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "profile_unavailable",
                "The change could not be confirmed. Retry the same save to recover its outcome.",
            )
        }
    };
    renew(&mut response, session.renewal);
    response
}
