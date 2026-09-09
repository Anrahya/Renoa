use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use renoa_local::{LocalHostError, RoutineEnablement, RoutineError, TurnObservation};
use serde::Serialize;
use uuid::Uuid;

use crate::{ManagementError, ManagementState, authorize, failure};

pub(super) fn validate_origin(value: &str) -> Result<String, ManagementError> {
    let url = reqwest::Url::parse(value).map_err(|_| ManagementError::InvalidOrigin)?;
    if url.host_str().is_none()
        || !(url.scheme() == "https"
            || (url.scheme() == "http" && url.host_str() == Some("localhost")))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ManagementError::InvalidOrigin);
    }
    Ok(url.origin().ascii_serialization())
}

#[derive(Serialize)]
struct Receipt {
    operation_id: Uuid,
    id: Uuid,
    revision: i64,
    enabled: bool,
    next_due_ms: i64,
}

pub(super) async fn set_enabled(
    State(state): State<Arc<ManagementState>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    request: Result<Json<RoutineEnablement>, JsonRejection>,
) -> Response {
    // Never derive authority from Host/Forwarded headers or a model-supplied actor.
    let mut origins = headers.get_all(header::ORIGIN).iter();
    if origins
        .next()
        .is_none_or(|value| value != state.origin.as_str())
        || origins.next().is_some()
    {
        return failure(
            StatusCode::FORBIDDEN,
            "wrong_origin",
            "Open the control panel at its configured Host address.",
        );
    }
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let request = match request {
        Ok(Json(request)) => request,
        Err(error) => {
            return failure(
                error.status(),
                "invalid_request",
                "Send an operation ID, expected revision, and enabled state as JSON.",
            );
        }
    };
    let operation_id = request.operation_id;
    let now = match TurnObservation::now() {
        Ok(now) => now.unix_milliseconds(),
        Err(error) => {
            eprintln!("Host management clock failed: {error}");
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "clock_unavailable",
                "The Host clock is unavailable. Retry the same request.",
            );
        }
    };
    let mut response = match state
        .routines
        .set_enabled(session.principal.as_uuid(), id, request, now)
        .await
    {
        Ok(record) => Json(Receipt {
            operation_id,
            id: record.id,
            revision: record.revision,
            enabled: record.spec.enabled,
            next_due_ms: record.next_due_ms,
        })
        .into_response(),
        Err(LocalHostError::Routine(RoutineError::Conflict)) => failure(
            StatusCode::CONFLICT,
            "revision_conflict",
            "This automation changed or the operation ID was reused. Refresh before making a new change.",
        ),
        Err(LocalHostError::Routine(RoutineError::NotFound)) => failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "This automation no longer exists.",
        ),
        Err(LocalHostError::Routine(RoutineError::Forbidden)) => failure(
            StatusCode::FORBIDDEN,
            "wrong_owner",
            "This login does not own the configured Host.",
        ),
        Err(LocalHostError::Routine(RoutineError::Invalid(reason))) => failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_schedule",
            &reason,
        ),
        Err(error) => {
            eprintln!("Host management routine operation {operation_id} failed: {error}");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "host_unavailable",
                "The change could not be confirmed. Retry the same request to recover its outcome.",
            )
        }
    };
    if let Some(cookie) = session.renewal {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}
