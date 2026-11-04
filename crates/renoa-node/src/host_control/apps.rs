//! The apps an agent can use, listed and switched by the owner. Switching one
//! off keeps its sign-in; switching it on again needs none.

use renoa_kernel::AgentId;
use renoa_local::{AgentApp, LocalHost, LocalHostError, McpHostError};
use serde::Deserialize;
use uuid::Uuid;

use super::reply::{BAD_REQUEST, CONFLICT, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, log};

/// The state asked for. Asking twice is the same as asking once.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Switch {
    enabled: bool,
}

pub(super) async fn list(host: &LocalHost, id: &str) -> Reply {
    match agent(id) {
        Ok(agent) => answer(host.agent_apps(agent).await),
        Err(reply) => reply,
    }
}

pub(super) async fn set(host: &LocalHost, id: &str, connection: &str, switch: Switch) -> Reply {
    match agent(id) {
        Ok(agent) => answer(host.set_agent_app(agent, connection, switch.enabled).await),
        Err(reply) => reply,
    }
}

fn agent(id: &str) -> Result<AgentId, Reply> {
    Uuid::parse_str(id).map(AgentId::from_uuid).map_err(|_| {
        Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "An agent id is a UUID, such as 0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0.",
        )
    })
}

fn answer(outcome: Result<Vec<AgentApp>, LocalHostError>) -> Reply {
    #[derive(serde::Serialize)]
    struct Apps {
        apps: Vec<AgentApp>,
    }
    match outcome {
        Ok(apps) => Reply::json(&Apps { apps }),
        Err(LocalHostError::AgentNotFound(_)) => {
            Reply::failure(NOT_FOUND, "not_found", "This agent no longer exists.")
        }
        // An unknown connection, or one whose sign-in never finished.
        Err(LocalHostError::Mcp(McpHostError::NotFound(_))) => Reply::failure(
            NOT_FOUND,
            "app_not_found",
            "This app is not set up any more. Ask an agent to connect it again.",
        ),
        Err(LocalHostError::Mcp(McpHostError::Invalid(reason))) => {
            Reply::failure(UNPROCESSABLE, "invalid_app", &reason)
        }
        // The agent switched off or replaced the plugin that serves it.
        Err(LocalHostError::Mcp(McpHostError::Conflict(_))) => Reply::failure(
            CONFLICT,
            "app_unavailable",
            "This app can't be turned on from here. Ask the agent to turn it on.",
        ),
        Err(error) => {
            log("apps", &error.to_string());
            Reply::failure(
                UNAVAILABLE,
                "host_unavailable",
                "The apps could not be read or changed. Retry the same request.",
            )
        }
    }
}
