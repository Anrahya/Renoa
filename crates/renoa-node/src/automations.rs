//! The Host's automation schedule, run inside the node as an RCP surface.
//!
//! Each due run becomes a command, under the run's own identity, on the task
//! of the conversation its automation was created in, or else on a task of the
//! automation's own whose identity is the automation's. The command executes
//! on this node like any other, so every surface attached to the task sees it
//! and its result through the journal. Once the node ledger holds the
//! command's terminal event, the scheduler records the result on the run.
//!
//! A run keeps its identity until its result is recorded, so after a restart
//! the scheduler submits the same command again: the coordinator keeps one
//! copy, and the ledger re-drives an execution the restart interrupted.
//!
//! The Host decides whether a due occurrence is too late to run; a skipped
//! one is only reported here. While it owns the schedule, the scheduler
//! writes a heartbeat so Host observation can tell it is alive.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use renoa_control::{DeviceCredentials, ErrorCode, TaskId};
use renoa_local::{AutomationScheduler, RunOutcome, RunStatus, ScheduledRun, TurnObservation};
use renoa_protocol::{CommandId, ExecutionTerminal};
use renoa_rcp_client::{ClientError, Connection};
use tokio_util::sync::CancellationToken;

use crate::{
    agent_targets, automation_cleanup,
    backoff::{ReconnectBackoff, STABLE_CONNECTION},
    bridge::{NodeError, NodeRuntime},
    node_log,
    node_store::CommandOutcome,
};

/// How long the scheduler waits before looking for due work again, or before
/// retrying a submission the coordinator refused because the node is offline.
const PAUSE: Duration = Duration::from_secs(1);

/// How often the scheduler records that it is alive.
const HEARTBEAT: Duration = Duration::from_secs(30);

/// The surface link that submits this Host's automation runs, and the
/// schedule it owns.
pub(crate) struct AutomationLink {
    pub(crate) endpoint: String,
    pub(crate) credentials: DeviceCredentials,
    pub(crate) scheduler: AutomationScheduler,
}

/// Runs the schedule until shutdown, reconnecting the surface link with
/// bounded backoff, and writes the heartbeat throughout, since a lost
/// coordinator link does not stop the scheduler. Only socket loss is retried.
pub(crate) async fn run(
    runtime: Arc<NodeRuntime>,
    link: AutomationLink,
    shutdown: CancellationToken,
) -> Result<(), NodeError> {
    tokio::select! {
        result = schedule(&runtime, &link, &shutdown) => result,
        result = heartbeat(&link.scheduler, &shutdown) => result,
    }
}

async fn heartbeat(
    scheduler: &AutomationScheduler,
    shutdown: &CancellationToken,
) -> Result<(), NodeError> {
    let mut ticks = tokio::time::interval(HEARTBEAT);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            _ = ticks.tick() => {}
        }
        if let Err(error) = scheduler.heartbeat(now_ms()?).await {
            node_log::event(
                "warn",
                "automation_heartbeat_failed",
                &serde_json::json!({ "error": error.to_string() }),
            );
        }
    }
}

async fn schedule(
    runtime: &NodeRuntime,
    link: &AutomationLink,
    shutdown: &CancellationToken,
) -> Result<(), NodeError> {
    let mut backoff = ReconnectBackoff::new();
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        let mut connected_at = None;
        let ended = match renoa_rcp_client::connect(&link.endpoint, link.credentials.clone()).await
        {
            // No task is attached, so the event stream carries nothing.
            Ok((connection, _events)) => {
                connected_at = Some(Instant::now());
                serve(runtime, &link.scheduler, &connection, shutdown).await
            }
            Err(error) => Err(error.into()),
        };
        let reason = match ended {
            Ok(()) => return Ok(()),
            Err(NodeError::Transport(reason)) => reason,
            Err(error) => return Err(error),
        };
        let delay = backoff
            .next_delay(connected_at.is_some_and(|started| started.elapsed() >= STABLE_CONNECTION));
        node_log::event(
            "warn",
            "automation_link_disconnected",
            &serde_json::json!({
                "reason": reason,
                "retry_ms": u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            }),
        );
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            () = tokio::time::sleep(delay) => {}
        }
    }
}

async fn serve(
    runtime: &NodeRuntime,
    scheduler: &AutomationScheduler,
    connection: &Connection,
    shutdown: &CancellationToken,
) -> Result<(), NodeError> {
    let mut next_cleanup = Instant::now();
    loop {
        let Some(due) = scheduler
            .next_run(now_ms()?)
            .await
            .map_err(|error| host_error(&error))?
        else {
            // Deleted automations' conversations go while nothing is due.
            // Stopping part way is safe: every step converges on a retry and
            // the Host's mark goes last.
            if Instant::now() >= next_cleanup {
                tokio::select! {
                    () = shutdown.cancelled() => return Ok(()),
                    deleted = automation_cleanup::delete_conversations(
                        runtime, scheduler, connection,
                    ) => deleted?,
                }
                next_cleanup = Instant::now() + HEARTBEAT;
            }
            tokio::select! {
                () = shutdown.cancelled() => return Ok(()),
                () = connection.closed() => {
                    return Err(ClientError::Transport(
                        "the coordinator connection ended".to_owned(),
                    ).into());
                }
                () = tokio::time::sleep(PAUSE) => continue,
            }
        };
        if due.run.result.as_ref().map(|result| result.status) == Some(RunStatus::Skipped) {
            node_log::event(
                "warn",
                "automation_skipped",
                &serde_json::json!({
                    "automation_id": due.run.automation_id,
                    "run_id": due.run.id,
                    "due_ms": due.run.due_ms,
                    "late_ms": due.run.admitted_at_ms.saturating_sub(due.run.due_ms),
                }),
            );
            continue;
        }
        let Some(outcome) = deliver(runtime, connection, &due, shutdown).await? else {
            return Ok(());
        };
        let fields = serde_json::json!({
            "automation_id": due.run.automation_id,
            "run_id": due.run.id,
            "status": outcome.status(),
            "failed_tool_calls": outcome.failed_tool_calls(),
        });
        scheduler
            .finish_run(due.run.id, outcome, now_ms()?)
            .await
            .map_err(|error| host_error(&error))?;
        node_log::event("info", "automation_finished", &fields);
    }
}

/// Submits one run and waits for its result. Returns `None` at shutdown; the
/// run stays admitted and is delivered again after the restart.
async fn deliver(
    runtime: &NodeRuntime,
    connection: &Connection,
    due: &ScheduledRun,
    shutdown: &CancellationToken,
) -> Result<Option<RunOutcome>, NodeError> {
    let run = &due.run;
    let task_id = match task_for(runtime, connection, due, shutdown).await? {
        Resolution::Task(task_id) => task_id,
        Resolution::Refused(reason) => return Ok(Some(refused(due, None, &reason))),
        Resolution::Shutdown => return Ok(None),
    };
    let command_id = CommandId::from_uuid(run.id);
    loop {
        match connection
            .submit(task_id, command_id, run.submission.clone())
            .await
        {
            Ok(()) => break,
            Err(error) if error.code() == Some(ErrorCode::NodeOffline) => {
                if !pause(shutdown).await {
                    return Ok(None);
                }
            }
            Err(error @ ClientError::Rejected { .. }) => {
                return Ok(Some(refused(due, Some(task_id), &error.to_string())));
            }
            Err(error) => return Err(error.into()),
        }
    }
    node_log::event(
        "info",
        "automation_submitted",
        &serde_json::json!({
            "automation_id": run.automation_id,
            "run_id": run.id,
            "command_id": command_id,
            "task_id": task_id,
            "due_ms": run.due_ms,
            "late_ms": run.admitted_at_ms.saturating_sub(run.due_ms),
        }),
    );
    Ok(wait_for_outcome(runtime, command_id, shutdown)
        .await?
        .map(finished))
}

enum Resolution {
    Task(TaskId),
    Refused(String),
    Shutdown,
}

/// The task a run is submitted to: the task executing in the conversation
/// its automation was created in, or else the automation's own task, opened
/// on this node when the run first needs it.
async fn task_for(
    runtime: &NodeRuntime,
    connection: &Connection,
    due: &ScheduledRun,
    shutdown: &CancellationToken,
) -> Result<Resolution, NodeError> {
    if let Some(session) = due.origin_session_id
        && let Some(task_id) = runtime.state.task_for_session(session).await?
    {
        return Ok(Resolution::Task(task_id));
    }
    let agent_id = due.run.agent_id;
    let task_id = TaskId::from_uuid(due.run.automation_id);
    let target = agent_targets::target(agent_id);
    loop {
        let nodes = connection
            .list_targets()
            .await?
            .into_iter()
            .filter(|summary| summary.target == target)
            .map(|summary| summary.node_id)
            .collect::<Vec<_>>();
        match nodes.as_slice() {
            [node_id] => match connection
                .open_task(task_id, *node_id, target.clone())
                .await
            {
                Ok(()) => return Ok(Resolution::Task(task_id)),
                Err(error) if error.code() == Some(ErrorCode::NodeOffline) => {}
                Err(error @ ClientError::Rejected { .. }) => {
                    return Ok(Resolution::Refused(error.to_string()));
                }
                Err(error) => return Err(error.into()),
            },
            // This node advertises every Host agent soon after it connects, so
            // only an agent the Host no longer has stays unadvertised.
            [] => {
                let exists = runtime
                    .host
                    .agent_definition(agent_id)
                    .await
                    .map_err(|error| host_error(&error))?
                    .is_some();
                if !exists {
                    return Ok(Resolution::Refused(format!(
                        "agent {agent_id} no longer exists"
                    )));
                }
            }
            _ => {
                return Ok(Resolution::Refused(format!(
                    "more than one node advertises `{}`",
                    target.as_str()
                )));
            }
        }
        if !pause(shutdown).await {
            return Ok(Resolution::Shutdown);
        }
    }
}

async fn wait_for_outcome(
    runtime: &NodeRuntime,
    command_id: CommandId,
    shutdown: &CancellationToken,
) -> Result<Option<CommandOutcome>, NodeError> {
    let mut commits = runtime.commits.subscribe();
    loop {
        if let Some(outcome) = runtime.state.outcome(command_id).await? {
            return Ok(Some(outcome));
        }
        tokio::select! {
            () = shutdown.cancelled() => return Ok(None),
            changed = commits.changed() => changed.map_err(|_| {
                NodeError::Protocol("local commit signal closed".to_owned())
            })?,
        }
    }
}

fn finished(outcome: CommandOutcome) -> RunOutcome {
    let failed_tool_calls = outcome.failed_tool_calls;
    match outcome.terminal {
        ExecutionTerminal::Completed => RunOutcome::Succeeded {
            answer: outcome.answer.unwrap_or_default(),
            failed_tool_calls,
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
fn refused(due: &ScheduledRun, task_id: Option<TaskId>, reason: &str) -> RunOutcome {
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

fn now_ms() -> Result<i64, NodeError> {
    Ok(TurnObservation::now()
        .map_err(|error| NodeError::Task(error.to_string()))?
        .unix_milliseconds())
}

/// Waits before a retry. Returns `false` when the node is shutting down.
async fn pause(shutdown: &CancellationToken) -> bool {
    tokio::select! {
        () = shutdown.cancelled() => false,
        () = tokio::time::sleep(PAUSE) => true,
    }
}

fn host_error(error: &renoa_local::LocalHostError) -> NodeError {
    NodeError::Store(error.to_string())
}

impl From<ClientError> for NodeError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Transport(reason) => Self::Transport(reason),
            ClientError::Rejected { code, message } => Self::Rejected { code, message },
            ClientError::Protocol(reason) => Self::Protocol(reason),
        }
    }
}
