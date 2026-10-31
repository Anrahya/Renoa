//! The owner naming the Host's main agent.

use renoa_local::{HostMainAgentControl, LocalHost, MainAgentChange, MainAgentError};
use renoa_protocol::PrincipalId;
use uuid::Uuid;

use super::reply::{CONFLICT, NOT_FOUND, Reply, UNAVAILABLE, log};

pub(super) async fn set(
    host: &LocalHost,
    host_id: Uuid,
    principal: PrincipalId,
    change: MainAgentChange,
) -> Reply {
    let operation_id = change.operation_id;
    // The coordinator authenticated this principal and checked that it owns
    // this node; the control is opened for that principal, as automations are.
    let control = match HostMainAgentControl::open(host.home().path(), host_id, principal.as_uuid())
    {
        Ok(control) => control,
        Err(error) => return unconfirmed(operation_id, &error.to_string()),
    };
    match control.set(principal.as_uuid(), change).await {
        Ok(main_agent) => Reply::json(&main_agent),
        Err(MainAgentError::Stale) => Reply::failure(
            CONFLICT,
            "revision_conflict",
            "The main agent changed since you read it. Refresh before choosing again.",
        ),
        Err(MainAgentError::OperationReused) => Reply::failure(
            CONFLICT,
            "operation_conflict",
            "This operation ID was already used for a different main agent change.",
        ),
        Err(MainAgentError::AgentNotFound(_)) => {
            Reply::failure(NOT_FOUND, "not_found", "This agent is not on the Host.")
        }
        Err(error) => unconfirmed(operation_id, &error.to_string()),
    }
}

fn unconfirmed(operation_id: Uuid, error: &str) -> Reply {
    log(
        "main agent change",
        &format!("operation {operation_id}: {error}"),
    );
    Reply::failure(
        UNAVAILABLE,
        "host_unavailable",
        "The change could not be confirmed. Retry the same request to recover its outcome.",
    )
}
