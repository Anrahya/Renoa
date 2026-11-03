//! How often an agent's heartbeat checks in, read and set by the owner.

use renoa_kernel::AgentId;
use renoa_local::{LocalHost, LocalHostError, PluginError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::reply::{BAD_REQUEST, CONFLICT, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, log};

/// A change, retried under its operation ID.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Change {
    operation_id: Uuid,
    heartbeat_hours: u8,
}

/// The heartbeat as it is: how many hours apart it runs, 0 when off.
#[derive(Serialize)]
struct Heartbeat {
    heartbeat_hours: u8,
}

pub(super) async fn read(host: &LocalHost, id: &str) -> Reply {
    match agent(id) {
        Ok(agent) => answer(None, host.heartbeat_hours(agent).await),
        Err(reply) => reply,
    }
}

pub(super) async fn set(host: &LocalHost, id: &str, change: Change) -> Reply {
    match agent(id) {
        Ok(agent) => answer(
            Some(change.operation_id),
            host.set_heartbeat_hours(agent, change.heartbeat_hours, change.operation_id)
                .await,
        ),
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

fn answer(operation: Option<Uuid>, outcome: Result<Option<u8>, LocalHostError>) -> Reply {
    match outcome {
        Ok(Some(heartbeat_hours)) => Reply::json(&Heartbeat { heartbeat_hours }),
        Ok(None) => Reply::failure(NOT_FOUND, "not_found", "This agent no longer exists."),
        Err(LocalHostError::Plugin(PluginError::Invalid(reason))) => {
            Reply::failure(UNPROCESSABLE, "invalid_heartbeat", &reason)
        }
        Err(LocalHostError::Plugin(PluginError::Conflict(_))) => Reply::failure(
            CONFLICT,
            "operation_reused",
            "This operation ID was used for another change. Send a new one.",
        ),
        Err(error) => {
            let operation = operation.map_or_else(String::new, |id| format!("operation {id}: "));
            log("heartbeat", &format!("{operation}{error}"));
            Reply::failure(
                UNAVAILABLE,
                "host_unavailable",
                "The heartbeat could not be read or changed. Retry the same request.",
            )
        }
    }
}
