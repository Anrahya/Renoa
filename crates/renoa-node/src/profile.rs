use renoa_core::{CapabilityOutcome, RunEvent, RunEventKind, TerminalState};
use renoa_protocol::{ExecutionEvent, ExecutionEventKind, ExecutionTerminal};

pub(crate) fn into_execution_event(event: RunEvent) -> ExecutionEvent {
    ExecutionEvent {
        event_id: event.event_id,
        execution_id: event.run_id,
        sequence: event.sequence,
        recorded_at_ms: event.recorded_at_ms,
        kind: match event.kind {
            RunEventKind::RunStarted { .. } => ExecutionEventKind::ExecutionStarted,
            RunEventKind::ModelRequested { .. } => ExecutionEventKind::TurnStarted,
            RunEventKind::ModelResponded { response, .. } => ExecutionEventKind::AssistantMessage {
                text: response.text,
            },
            RunEventKind::CapabilityRequested { call, .. } => ExecutionEventKind::ToolStarted {
                call_id: call.call_id,
                name: call.name,
                arguments: call.arguments,
            },
            RunEventKind::CapabilityCompleted {
                call_id, outcome, ..
            } => ExecutionEventKind::ToolFinished {
                call_id,
                output: model_output(&outcome),
                is_error: outcome.is_error,
            },
            RunEventKind::RunTerminated { terminal } => ExecutionEventKind::ExecutionTerminated {
                terminal: match terminal {
                    TerminalState::Completed { .. } => ExecutionTerminal::Completed,
                    TerminalState::Failed { error } => ExecutionTerminal::Failed { error },
                    TerminalState::Cancelled { reason } => ExecutionTerminal::Cancelled { reason },
                },
            },
        },
    }
}

fn model_output(outcome: &CapabilityOutcome) -> String {
    outcome.model_view.as_str().map_or_else(
        || serde_json::to_string(&outcome.model_view).expect("JSON value is serializable"),
        ToOwned::to_owned,
    )
}
