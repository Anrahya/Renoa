use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, LocalHostError, ModelProvider,
    ReasoningLevel,
};
use serde::Serialize;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Serialize)]
struct CatalogEntry {
    provider: ModelProvider,
    model: String,
    name: String,
    reasoning_levels: Vec<ReasoningLevel>,
    default_reasoning: Option<ReasoningLevel>,
}

#[derive(Serialize)]
struct Options {
    native_tools: Vec<&'static str>,
    models: Vec<CatalogEntry>,
    default_model: renoa_local::AgentModelSelection,
}

use crate::{ManagementState, authorize, failure, origin_failure};

pub(super) async fn definition(
    State(state): State<Arc<ManagementState>>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
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
    let mut response = match host
        .agent_definition(renoa_kernel::AgentId::from_uuid(id))
        .await
    {
        Ok(Some(record)) => Json(record).into_response(),
        Ok(None) => failure(
            StatusCode::NOT_FOUND,
            "not_found",
            "This agent is not on the Host.",
        ),
        Err(_) => unavailable(),
    };
    renew(&mut response, session.renewal);
    response
}
pub(super) async fn options(
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
    let Ok(models) = host.agent_creation_models().await else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "models_unavailable",
            "The provider catalog is unavailable. Retry before creating an agent.",
        );
    };
    let mut response = Json(Options {
        native_tools: host.selectable_native_tools(),
        default_model: host.default_agent_model(),
        models: models
            .into_iter()
            .map(|model| CatalogEntry {
                provider: model.provider(),
                model: model.id().into(),
                name: model.name().into(),
                reasoning_levels: model.reasoning_levels().to_vec(),
                default_reasoning: model.default_reasoning(),
            })
            .collect(),
    })
    .into_response();
    renew(&mut response, session.renewal);
    response
}

pub(super) async fn create(
    State(state): State<Arc<ManagementState>>,
    headers: HeaderMap,
    request: Result<Json<AgentCreateRequest>, JsonRejection>,
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
                "Send a valid agent creation request.",
            );
        }
    };
    let operation_id = request.operation_id;
    let creator = AgentCreator::Principal {
        host_id: state.observer.host_id(),
        principal_id: session.principal.to_string(),
    };
    let mut response = match host
        .create_agent(
            creator,
            AgentCreationOrigin::Management,
            request,
            CancellationToken::new(),
        )
        .await
    {
        Ok(record) => Json(serde_json::json!({ "operation_id": operation_id, "record": record }))
            .into_response(),
        Err(LocalHostError::AgentConflict(_)) => failure(
            StatusCode::CONFLICT,
            "operation_conflict",
            "This operation ID was already used for a different creation request.",
        ),
        Err(LocalHostError::Definition(error)) => failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_agent",
            &error.to_string(),
        ),
        Err(LocalHostError::InvalidRequest(reason) | LocalHostError::Configuration(reason)) => {
            failure(StatusCode::UNPROCESSABLE_ENTITY, "invalid_agent", &reason)
        }
        Err(_) => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "host_unavailable",
            "Creation could not be confirmed. Retry the same saved request.",
        ),
    };
    renew(&mut response, session.renewal);
    response
}

pub(super) fn unavailable() -> Response {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "creation_unavailable",
        "Agent creation is not configured on this Host.",
    )
}
pub(super) fn renew(response: &mut Response, cookie: Option<axum::http::HeaderValue>) {
    if let Some(cookie) = cookie {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
}
