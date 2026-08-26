use crate::{LocalHostError, LocalTurnOutcome, trace::TraceRun};

pub(crate) async fn finish_trace(
    trace: &TraceRun,
    result: &Result<LocalTurnOutcome, LocalHostError>,
) {
    let (status, error_code, error_message, payload) = match result {
        Ok(LocalTurnOutcome::Completed {
            output,
            stop_reason,
        }) => (
            "completed",
            None,
            None,
            serde_json::json!({
                "outcome": "completed",
                "output": output,
                "stop_reason": stop_reason
            }),
        ),
        Ok(LocalTurnOutcome::Compacted {
            estimated_input_tokens,
        }) => (
            "completed",
            None,
            None,
            serde_json::json!({
                "outcome": "compacted",
                "estimated_input_tokens": estimated_input_tokens
            }),
        ),
        Ok(LocalTurnOutcome::Cancelled) => (
            "cancelled",
            Some("cancelled"),
            None,
            serde_json::json!({ "outcome": "cancelled" }),
        ),
        Ok(LocalTurnOutcome::Failed { reason }) => (
            "failed",
            Some("operation_failed"),
            Some(reason.clone()),
            serde_json::json!({ "outcome": "failed", "reason": reason }),
        ),
        Ok(LocalTurnOutcome::WaitingForInput) => (
            "waiting_for_input",
            None,
            None,
            serde_json::json!({ "outcome": "waiting_for_input" }),
        ),
        Err(error) => (
            "failed",
            Some(host_error_code(error)),
            Some(error.to_string()),
            serde_json::json!({
                "outcome": "error",
                "code": host_error_code(error),
                "message": error.to_string()
            }),
        ),
    };
    if let Err(error) = trace
        .record_host("turn_finished", Some(status), payload)
        .await
    {
        eprintln!("Renoa trace record failed: {error}");
    }
    if let Err(error) = trace
        .finish(status, error_code, error_message.as_deref())
        .await
    {
        eprintln!("Renoa trace finalization failed: {error}");
    }
}

const fn host_error_code(error: &LocalHostError) -> &'static str {
    match error {
        LocalHostError::InvalidRequest(_) => "invalid_request",
        LocalHostError::Configuration(_) => "configuration",
        LocalHostError::Io(_) => "io",
        LocalHostError::Metadata(_) => "metadata",
        LocalHostError::Workspace(_) => "workspace",
        LocalHostError::Runtime(_) => "runtime",
        LocalHostError::Model(_) => "model",
        LocalHostError::Alpha(_) => "alpha",
        LocalHostError::Session(_) => "session",
        LocalHostError::Background(_) => "background",
        LocalHostError::StatePoisoned => "state_poisoned",
        LocalHostError::Trace(_) => "trace",
        LocalHostError::SessionCreationCleanup { .. } => "session_creation_cleanup",
    }
}
