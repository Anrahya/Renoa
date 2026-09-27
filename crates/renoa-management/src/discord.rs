//! Owner Discord setup: connect the Host's bot, then route channels to agents.

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response},
};
use renoa_discord::{DiscordBindingRequest, DiscordConnectRequest, DiscordError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ManagementState,
    agents::{renew, unavailable},
    authorize, failure, origin_failure,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inspection {
    bot_token: String,
}

pub(super) async fn status(
    State(state): State<Arc<ManagementState>>,
    headers: HeaderMap,
) -> Response {
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let mut response = reply(
        state.discord.status(),
        "Discord settings cannot be read. Saved settings are preserved.",
    );
    renew(&mut response, session.renewal);
    response
}

pub(super) async fn channels(
    State(state): State<Arc<ManagementState>>,
    headers: HeaderMap,
) -> Response {
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let mut response = reply(
        state.discord.channels().await,
        "Discord channels could not be read. Retry when Discord is reachable.",
    );
    renew(&mut response, session.renewal);
    response
}

/// Reads what a token reaches. The token is not stored.
pub(super) async fn inspect(
    State(state): State<Arc<ManagementState>>,
    headers: HeaderMap,
    request: Result<Json<Inspection>, JsonRejection>,
) -> Response {
    if let Some(response) = origin_failure(&state, &headers) {
        return response;
    }
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let mut response = match request {
        Ok(Json(request)) => reply(
            state.discord.inspect(request.bot_token).await,
            "Discord could not be reached. Retry the token check.",
        ),
        Err(error) => invalid(&error),
    };
    renew(&mut response, session.renewal);
    response
}

pub(super) async fn connect(
    State(state): State<Arc<ManagementState>>,
    headers: HeaderMap,
    request: Result<Json<DiscordConnectRequest>, JsonRejection>,
) -> Response {
    if let Some(response) = origin_failure(&state, &headers) {
        return response;
    }
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(host) = &state.agents else {
        return unavailable();
    };
    let mut response = match request {
        Ok(Json(request)) => {
            let operation_id = request.operation_id;
            receipt(
                operation_id,
                state.discord.connect(host, request).await,
                "The Discord connection could not be confirmed. Refresh to see whether it was saved.",
            )
        }
        Err(error) => invalid(&error),
    };
    renew(&mut response, session.renewal);
    response
}

pub(super) async fn bind(
    State(state): State<Arc<ManagementState>>,
    headers: HeaderMap,
    request: Result<Json<DiscordBindingRequest>, JsonRejection>,
) -> Response {
    if let Some(response) = origin_failure(&state, &headers) {
        return response;
    }
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(host) = &state.agents else {
        return unavailable();
    };
    let mut response = match request {
        Ok(Json(request)) => {
            let operation_id = request.operation_id;
            receipt(
                operation_id,
                state.discord.bind(host, request).await,
                "Channel binding could not be confirmed. Check bot access, then retry the saved request.",
            )
        }
        Err(error) => invalid(&error),
    };
    renew(&mut response, session.renewal);
    response
}

fn receipt<T: Serialize>(
    operation_id: Uuid,
    result: Result<T, DiscordError>,
    unknown: &str,
) -> Response {
    reply(
        result.map(|record| serde_json::json!({ "operation_id": operation_id, "record": record })),
        unknown,
    )
}

/// Discord's own rejection explains itself; anything else leaves the outcome unknown.
fn reply<T: Serialize>(result: Result<T, DiscordError>, unknown: &str) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(DiscordError::Invalid(reason)) => {
            failure(StatusCode::CONFLICT, "discord_rejected", &reason)
        }
        Err(error) => {
            eprintln!("Host management Discord request failed: {error}");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "discord_unavailable",
                unknown,
            )
        }
    }
}

fn invalid(error: &JsonRejection) -> Response {
    failure(
        error.status(),
        "invalid_request",
        "Send a valid Discord request.",
    )
}
