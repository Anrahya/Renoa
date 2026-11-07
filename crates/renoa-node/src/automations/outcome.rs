//! How a delivered run's ending becomes the outcome the Host records.

use renoa_local::{AutomationRun, RunLanding, RunOutcome, ScheduledRun};
use renoa_protocol::ExecutionTerminal;
use renoa_rcp_wire::TaskId;

use crate::{node_log, node_store::CommandOutcome};

/// The outcome of `run`, which executed on `task_id`. Only this node knows
/// where it landed: the automation's own task is named by the automation, and
/// any other is the conversation it was created in, where its result already
/// shows.
pub(super) fn finished(
    outcome: CommandOutcome,
    run: &AutomationRun,
    task_id: TaskId,
) -> RunOutcome {
    let failed_tool_calls = outcome.failed_tool_calls;
    match outcome.terminal {
        ExecutionTerminal::Completed => RunOutcome::Succeeded {
            answer: outcome.answer.unwrap_or_default(),
            failed_tool_calls,
            landing: if task_id == TaskId::from_uuid(run.automation_id) {
                RunLanding::OwnTask
            } else {
                RunLanding::Conversation
            },
        },
        ExecutionTerminal::Failed { error } => RunOutcome::Failed {
            reason: format!("Scheduled run failed: {error}"),
            failed_tool_calls,
        },
        ExecutionTerminal::Cancelled { reason } => RunOutcome::Failed {
            reason: format!("Scheduled run stopped: {reason}"),
            failed_tool_calls,
        },
    }
}

/// A run the coordinator would not accept ends with the refusal as its result,
/// so it is visible where results are read and the schedule moves on.
pub(super) fn refused(due: &ScheduledRun, task_id: Option<TaskId>, reason: &str) -> RunOutcome {
    node_log::event(
        "warn",
        "automation_refused",
        &serde_json::json!({
            "automation_id": due.run.automation_id,
            "run_id": due.run.id,
            "task_id": task_id,
            "error": reason,
        }),
    );
    RunOutcome::Failed {
        reason: format!("Scheduled run could not be sent: {reason}"),
        failed_tool_calls: 0,
    }
}
