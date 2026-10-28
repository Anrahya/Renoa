//! Owner pause and resume of an existing automation.

use renoa_local::{
    AutomationEnablement, AutomationError, HostAutomationControl, LocalHost, LocalHostError,
    TurnObservation,
};
use renoa_protocol::PrincipalId;
use serde::Serialize;
use uuid::Uuid;

use super::reply::{
    BAD_REQUEST, CONFLICT, FORBIDDEN, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, log,
};

#[derive(Serialize)]
struct Receipt {
    operation_id: Uuid,
    id: Uuid,
    revision: i64,
    enabled: bool,
    next_due_ms: i64,
}

pub(super) async fn set_enabled(
    host: &LocalHost,
    host_id: Uuid,
    principal: PrincipalId,
    id: &str,
    request: AutomationEnablement,
) -> Reply {
    let Ok(id) = Uuid::parse_str(id) else {
        return Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "An automation id is a UUID, such as 0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0.",
        );
    };
    let operation_id = request.operation_id;
    let now = match TurnObservation::now() {
        Ok(now) => now.unix_milliseconds(),
        Err(error) => {
            log("automation clock", &error.to_string());
            return Reply::failure(
                UNAVAILABLE,
                "clock_unavailable",
                "The Host clock is unavailable. Retry the same request.",
            );
        }
    };
    // The coordinator authenticated this principal as the Host's owner.
    let control =
        match HostAutomationControl::open(host.home().path(), host_id, principal.as_uuid()) {
            Ok(control) => control,
            Err(error) => return unconfirmed(operation_id, &error),
        };
    match control
        .set_enabled(principal.as_uuid(), id, request, now)
        .await
    {
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
        Err(LocalHostError::Automation(AutomationError::NotFound)) => {
            Reply::failure(NOT_FOUND, "not_found", "This automation no longer exists.")
        }
        Err(LocalHostError::Automation(AutomationError::Forbidden)) => Reply::failure(
            FORBIDDEN,
            "wrong_owner",
            "This login does not own the configured Host.",
        ),
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
