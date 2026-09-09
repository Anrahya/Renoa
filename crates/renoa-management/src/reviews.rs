use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use renoa_local::{GitHubReviewError, LocalHostError, ReviewPolicyUpdate};

use crate::{ManagementState, authorize, failure, origin_failure};

pub(crate) async fn update_policy(
    State(state): State<Arc<ManagementState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    request: Result<Json<ReviewPolicyUpdate>, JsonRejection>,
) -> Response {
    if let Some(response) = origin_failure(&state, &headers) {
        return response;
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
                "Send an operation ID, expected revision, enabled state, triggers and draft policy as JSON.",
            );
        }
    };
    let operation_id = request.operation_id;
    let mut response = match state
        .reviews
        .update_policy(session.principal.as_uuid(), id, request)
        .await
    {
        Ok(record) => Json(serde_json::json!({"operation_id": operation_id, "record": record}))
            .into_response(),
        Err(LocalHostError::GitHubReview(GitHubReviewError::Conflict)) => failure(
            StatusCode::CONFLICT,
            "revision_conflict",
            "This review policy changed. Refresh before making a new change.",
        ),
        Err(LocalHostError::GitHubReview(GitHubReviewError::NotFound)) => failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "This repository is no longer configured.",
        ),
        Err(LocalHostError::GitHubReview(GitHubReviewError::Forbidden)) => failure(
            StatusCode::FORBIDDEN,
            "wrong_owner",
            "This login does not own the configured Host.",
        ),
        Err(LocalHostError::GitHubReview(GitHubReviewError::Invalid(reason))) => {
            failure(StatusCode::UNPROCESSABLE_ENTITY, "invalid_policy", &reason)
        }
        Err(error) => {
            eprintln!("Host review policy operation {operation_id} failed: {error}");
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
