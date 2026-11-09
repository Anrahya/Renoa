//! The apps an agent can use, listed and switched by the owner. Switching one
//! off takes its tools away and keeps its sign-in; switching it on again needs
//! none.

use renoa_local::{AgentApp, LocalHost, LocalHostError, McpHostError};
use serde::Deserialize;

use super::reply::{CONFLICT, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, agent_id, log};

/// The state asked for. Asking twice is the same as asking once.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Switch {
    enabled: bool,
}

pub(super) async fn list(host: &LocalHost, id: &str) -> Reply {
    match agent_id(id) {
        Ok(agent) => answer(host.agent_apps(agent).await),
        Err(reply) => reply,
    }
}

pub(super) async fn set(host: &LocalHost, id: &str, connection: &str, switch: Switch) -> Reply {
    match agent_id(id) {
        Ok(agent) => answer(host.set_agent_app(agent, connection, switch.enabled).await),
        Err(reply) => reply,
    }
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
        // The plugin that serves it is switched off, replaced or not admitted
        // for this agent.
        Err(LocalHostError::Mcp(McpHostError::Conflict(_))) => Reply::failure(
            CONFLICT,
            "app_unavailable",
            "This app can't be turned on from here. Ask the agent to set it up again.",
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
