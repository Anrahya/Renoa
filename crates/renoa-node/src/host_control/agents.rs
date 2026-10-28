//! Agent options, definitions and owner creation.

use renoa_kernel::AgentId;
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, LocalHost, LocalHostError,
    ModelProvider, ReasoningLevel,
};
use renoa_protocol::PrincipalId;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::reply::{BAD_REQUEST, CONFLICT, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, log};

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

pub(super) async fn definition(host: &LocalHost, id: &str) -> Reply {
    let Ok(id) = Uuid::parse_str(id) else {
        return Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "An agent id is a UUID, such as 0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0.",
        );
    };
    match host.agent_definition(AgentId::from_uuid(id)).await {
        Ok(Some(record)) => Reply::json(&record),
        Ok(None) => Reply::failure(NOT_FOUND, "not_found", "This agent is not on the Host."),
        Err(error) => {
            log("agent definition", &error.to_string());
            unavailable()
        }
    }
}

pub(super) async fn options(host: &LocalHost) -> Reply {
    let Ok(models) = host.agent_creation_models().await else {
        return Reply::failure(
            UNAVAILABLE,
            "models_unavailable",
            "The provider catalog is unavailable. Retry before creating an agent.",
        );
    };
    Reply::json(&Options {
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
}

pub(super) async fn create(
    host: &LocalHost,
    host_id: Uuid,
    principal: PrincipalId,
    request: AgentCreateRequest,
) -> Reply {
    let operation_id = request.operation_id;
    let creator = AgentCreator::Principal {
        host_id,
        principal_id: principal.to_string(),
    };
    match host
        .create_agent(
            creator,
            AgentCreationOrigin::Management,
            request,
            CancellationToken::new(),
        )
        .await
    {
        Ok(record) => {
            Reply::json(&serde_json::json!({ "operation_id": operation_id, "record": record }))
        }
        Err(LocalHostError::AgentConflict(_)) => Reply::failure(
            CONFLICT,
            "operation_conflict",
            "This operation ID was already used for a different creation request.",
        ),
        Err(LocalHostError::Definition(error)) => {
            Reply::failure(UNPROCESSABLE, "invalid_agent", &error.to_string())
        }
        Err(LocalHostError::InvalidRequest(reason) | LocalHostError::Configuration(reason)) => {
            Reply::failure(UNPROCESSABLE, "invalid_agent", &reason)
        }
        Err(error) => {
            log("agent creation", &error.to_string());
            Reply::failure(
                UNAVAILABLE,
                "host_unavailable",
                "Creation could not be confirmed. Retry the same saved request.",
            )
        }
    }
}

fn unavailable() -> Reply {
    Reply::failure(
        UNAVAILABLE,
        "host_unavailable",
        "Your Host could not read this agent. Retry in a moment.",
    )
}
