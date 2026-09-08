use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State as AxumState},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use renoa_local::{
    GitHubReviewError, GitHubReviewWebhook, LocalHost, LocalHostError, TurnObservation,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;

pub(super) struct State {
    pub host: LocalHost,
    pub secret: Vec<u8>,
    pub stop: CancellationToken,
}

pub(super) fn router(state: Arc<State>) -> Router {
    Router::new()
        .route("/v1/github/webhook", post(receive))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(state)
}

async fn receive(
    AxumState(state): AxumState<Arc<State>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let single = |name| {
        let mut values = headers.get_all(name).iter();
        let value = values.next()?.to_str().ok()?;
        if values.next().is_some() {
            None
        } else {
            Some(value)
        }
    };
    let (Some(delivery), Some(event), Some(signature)) = (
        single("x-github-delivery"),
        single("x-github-event"),
        single("x-hub-signature-256"),
    ) else {
        return StatusCode::BAD_REQUEST;
    };
    let Ok(delivery_id) = Uuid::parse_str(delivery) else {
        return StatusCode::BAD_REQUEST;
    };
    let Ok(now) = TurnObservation::now() else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    match state
        .host
        .admit_github_review_webhook(
            GitHubReviewWebhook {
                delivery_id,
                event,
                signature,
                body: &body,
            },
            &state.secret,
            now.unix_milliseconds(),
            state.stop.clone(),
        )
        .await
    {
        Ok(admission) => {
            eprintln!(
                "GitHub delivery {delivery_id}: {}",
                serde_json::to_string(&admission).unwrap_or_default()
            );
            StatusCode::ACCEPTED
        }
        Err(LocalHostError::GitHubReview(GitHubReviewError::Authentication)) => {
            StatusCode::UNAUTHORIZED
        }
        Err(LocalHostError::GitHubReview(
            GitHubReviewError::Invalid(_) | GitHubReviewError::Json(_),
        )) => StatusCode::BAD_REQUEST,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Conflict)) => StatusCode::CONFLICT,
        Err(error) => {
            eprintln!("GitHub admission {delivery_id} failed: {error}");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}
