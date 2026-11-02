//! Owner pause, resume and run-now of an existing automation.

use renoa_local::{
    AutomationEnablement, AutomationError, AutomationRecord, AutomationRunRequest,
    HostAutomationControl, LocalHost, LocalHostError, TurnObservation,
};
use renoa_protocol::PrincipalId;
use serde::Serialize;
use uuid::Uuid;

use super::reply::{BAD_REQUEST, CONFLICT, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, log};

#[derive(Serialize)]
struct Receipt {
    operation_id: Uuid,
    id: Uuid,
    revision: i64,
    enabled: bool,
    next_due_ms: i64,
}

/// The automation a call names, the Host clock, and the owner control
/// opened for the relayed principal.
struct Call {
    id: Uuid,
    now: i64,
    control: HostAutomationControl,
}

pub(super) async fn set_enabled(
    host: &LocalHost,
    host_id: Uuid,
    principal: PrincipalId,
    id: &str,
    request: AutomationEnablement,
) -> Reply {
    let operation_id = request.operation_id;
    let call = match open(host, host_id, principal, id, operation_id) {
        Ok(call) => call,
        Err(reply) => return reply,
    };
    let outcome = call
        .control
        .set_enabled(principal.as_uuid(), call.id, request, call.now)
        .await;
    receipt(operation_id, outcome)
}

/// Admits a run now under the request's operation ID, which becomes the
/// run's identity: the feed's `retry` action.
pub(super) async fn run_now(
    host: &LocalHost,
    host_id: Uuid,
    principal: PrincipalId,
    id: &str,
    request: AutomationRunRequest,
) -> Reply {
    let operation_id = request.operation_id;
    let call = match open(host, host_id, principal, id, operation_id) {
        Ok(call) => call,
        Err(reply) => return reply,
    };
    let outcome = call
        .control
        .run_now(principal.as_uuid(), call.id, request, call.now)
        .await;
    receipt(operation_id, outcome)
}

fn open(
    host: &LocalHost,
    host_id: Uuid,
    principal: PrincipalId,
    id: &str,
    operation_id: Uuid,
) -> Result<Call, Reply> {
    let Ok(id) = Uuid::parse_str(id) else {
        return Err(Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "An automation id is a UUID, such as 0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0.",
        ));
    };
    let now = match TurnObservation::now() {
        Ok(now) => now.unix_milliseconds(),
        Err(error) => {
            log("automation clock", &error.to_string());
            return Err(Reply::failure(
                UNAVAILABLE,
                "clock_unavailable",
                "The Host clock is unavailable. Retry the same request.",
            ));
        }
    };
    // The coordinator authenticated this principal and checked that it owns
    // this node; the node pins no owner of its own. The control is opened for
    // the relayed principal, so its owner check cannot refuse that principal.
    let control = HostAutomationControl::open(host.home().path(), host_id, principal.as_uuid())
        .map_err(|error| unconfirmed(operation_id, &error))?;
    Ok(Call { id, now, control })
}

fn receipt(operation_id: Uuid, outcome: Result<AutomationRecord, LocalHostError>) -> Reply {
    match outcome {
        Ok(record) => Reply::json(&Receipt {
            operation_id,
            id: record.id,
            revision: record.revision,
            enabled: record.spec.enabled,
            next_due_ms: record.next_due_ms,
        }),
        Err(LocalHostError::Automation(AutomationError::Conflict)) => Reply::failure(
            CONFLICT,
            "revision_conflict",
            "This automation changed or the operation ID was reused. Refresh before making a new change.",
        ),
        Err(LocalHostError::Automation(AutomationError::Busy)) => Reply::failure(
            CONFLICT,
            "run_in_progress",
            "This automation already has a run waiting or running. Wait for it to finish.",
        ),
        Err(LocalHostError::Automation(AutomationError::NotFound)) => {
            Reply::failure(NOT_FOUND, "not_found", "This automation no longer exists.")
        }
        Err(LocalHostError::Automation(AutomationError::Invalid(reason))) => {
            Reply::failure(UNPROCESSABLE, "invalid_schedule", &reason)
        }
        Err(error) => unconfirmed(operation_id, &error),
    }
}

fn unconfirmed(operation_id: Uuid, error: &LocalHostError) -> Reply {
    log(
        "automation change",
        &format!("operation {operation_id}: {error}"),
    );
    Reply::failure(
        UNAVAILABLE,
        "host_unavailable",
        "The change could not be confirmed. Retry the same request to recover its outcome.",
    )
}
