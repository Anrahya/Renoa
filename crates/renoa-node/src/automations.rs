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
//! A heartbeat's run is submitted quiet, so what it writes on its task wakes
//! no one: only the items its tools write for the owner do.
//!
//! The Host decides whether a due occurrence is too late to run; a skipped
//! one is only reported here. A run of a compiled plugin's trigger is not a
//! message to the agent: the Host runs it in this process, beside this loop,
//! which goes on submitting agents' due runs meanwhile and runs one plugin
//! run at a time. A draining node stops admitting runs, stops the Host's note
//! passes, and waits for the plugin run in flight, so it is not left
//! unfinished under the runtime a deploy replaces; shutdown drops it, and the
//! next scheduler resumes it. While it owns the schedule and the schedule
//! is not failing, the scheduler writes a heartbeat, so Host observation can
//! tell it is alive.

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use renoa_control::{DeviceCredentials, ErrorCode, TaskId};
use renoa_local::{AutomationScheduler, RunOutcome, RunStatus, ScheduledRun, TurnObservation};
use renoa_protocol::{Author, CommandId, CommandInput};
use renoa_rcp_client::{ClientError, Connection};
use tokio_util::sync::CancellationToken;

mod failures;
mod outcome;
mod plugin_run;

use self::{
    failures::{Failures, beat, heartbeat},
    outcome::{finished, refused},
    plugin_run::{PluginTask, Retry},
};
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
    pub(crate) scheduler: Arc<AutomationScheduler>,
}

/// Runs the schedule until shutdown or `drain`, reconnecting the surface link
/// with bounded backoff, and writes the heartbeat while the schedule works.
/// Nothing the schedule meets ends the node or its turns.
///
/// Socket loss reconnects. A catalog busy past its timeout is asked again
/// after [`PAUSE`] for as long as it stays busy. Any other Host failure is
/// asked again too; from the [`failures::FAILURES`]th in a row, or after any
/// failure that stops the schedule, which starts it again, the schedule is
/// failing: it logs `automation_schedule_failing`, writes no heartbeat, and
/// waits twice as long each time, up to a minute. The first tick that goes
/// well writes the heartbeat again. A plugin run that failed is held back
/// longer after each failure in a row.
///
/// At `drain` it stops the Host's note passes and returns once the plugin
/// run in flight ended, or at shutdown, which drops it.
pub(crate) async fn run(
    runtime: Arc<NodeRuntime>,
    link: AutomationLink,
    shutdown: CancellationToken,
    drain: CancellationToken,
) {
    let failing = Arc::new(AtomicBool::new(false));
    let mut state = State {
        plugin: None,
        retry: Retry::default(),
        failures: Failures::new(Arc::clone(&failing)),
    };
    loop {
        let scheduled = tokio::select! {
            result = schedule(&runtime, &link, &shutdown, &mut state) => result,
            () = heartbeat(&link.scheduler, &shutdown, &failing) => Ok(()),
            () = drain.cancelled() => Ok(()),
        };
        let Err(error) = scheduled else { break };
        let delay = state.failures.stopped(&error);
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = drain.cancelled() => break,
            () = tokio::time::sleep(delay) => {}
        }
    }
    if let Some(task) = state.plugin.take()
        && drain.is_cancelled()
    {
        let drained = tokio::select! {
            result = plugin_run::drain(&runtime, task, &shutdown) => result,
            () = heartbeat(&link.scheduler, &shutdown, &failing) => Ok(()),
        };
        // A plugin run that failed is logged; the node drains its turns.
        if let Err(error) = drained {
            node_log::event(
                "error",
                "automation_plugin_run_failed",
                &serde_json::json!({ "error": error.to_string() }),
            );
        }
    }
}

/// What the schedule keeps across reconnects: the plugin run in flight
/// beside it, how long plugin runs are held back after failing, and the
/// Host failures it met in a row.
struct State {
    plugin: Option<PluginTask>,
    retry: Retry,
    failures: Failures,
}

impl State {
    /// A tick that went well: a schedule that was failing writes its
    /// heartbeat again at once.
    async fn worked(&mut self, scheduler: &AutomationScheduler) {
        if self.failures.cleared() {
            beat(scheduler).await;
        }
    }
}

async fn schedule(
    runtime: &Arc<NodeRuntime>,
    link: &AutomationLink,
    shutdown: &CancellationToken,
    state: &mut State,
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
                serve(runtime, &link.scheduler, &connection, shutdown, state).await
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
    runtime: &Arc<NodeRuntime>,
    scheduler: &Arc<AutomationScheduler>,
    connection: &Connection,
    shutdown: &CancellationToken,
    state: &mut State,
) -> Result<(), NodeError> {
    let mut next_cleanup = Instant::now();
    loop {
        // A plugin's run that failed stays admitted, and is handed out again
        // once its retry delay passed.
        if let Some(ended) = plugin_run::reap(&mut state.plugin).await {
            state.retry.ended(ended);
        }
        let plugins_held = state.plugin.is_some() || state.retry.holds();
        let due = match scheduler.next_run(now_ms()?, plugins_held).await {
            Ok(Some(due)) => due,
            Ok(None) => {
                state.worked(scheduler).await;
                // Deleted automations' conversations go while nothing is due.
                // Stopping part way is safe: every step converges on a retry
                // and the Host's mark goes last.
                if Instant::now() >= next_cleanup {
                    tokio::select! {
                        () = shutdown.cancelled() => return Ok(()),
                        deleted = automation_cleanup::delete_conversations(
                            runtime, scheduler, connection,
                        ) => deleted?,
                    }
                    next_cleanup = Instant::now() + HEARTBEAT;
                }
                if !rest(connection, shutdown, PAUSE).await? {
                    return Ok(());
                }
                continue;
            }
            Err(error) => {
                let delay = state.failures.met(&error);
                if !rest(connection, shutdown, delay).await? {
                    return Ok(());
                }
                continue;
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
            state.worked(scheduler).await;
            continue;
        }
        if let Some(owner) = due.plugin {
            state.plugin = Some(plugin_run::start(runtime, scheduler, due, owner)?);
            state.worked(scheduler).await;
            continue;
        }
        let delivered = deliver(runtime, connection, &due, shutdown, &mut state.failures).await?;
        let Some(outcome) = delivered else {
            return Ok(());
        };
        let fields = serde_json::json!({
            "automation_id": due.run.automation_id,
            "run_id": due.run.id,
            "status": outcome.status(),
            "failed_tool_calls": outcome.failed_tool_calls(),
        });
        // A result the Host could not record leaves the run admitted: the
        // next tick submits the same command again, and the coordinator and
        // the ledger hand back the outcome it already has.
        if let Err(error) = scheduler.finish_run(due.run.id, outcome, now_ms()?).await {
            let delay = state.failures.met(&error);
            if !rest(connection, shutdown, delay).await? {
                return Ok(());
            }
            continue;
        }
        state.worked(scheduler).await;
        node_log::event("info", "automation_finished", &fields);
    }
}

/// Waits `delay` before the schedule's next tick. Returns `false` at
/// shutdown, and the transport failure when the coordinator link ended.
async fn rest(
    connection: &Connection,
    shutdown: &CancellationToken,
    delay: Duration,
) -> Result<bool, NodeError> {
    tokio::select! {
        () = shutdown.cancelled() => Ok(false),
        () = connection.closed() => Err(ClientError::Transport(
            "the coordinator connection ended".to_owned(),
        ).into()),
        () = tokio::time::sleep(delay) => Ok(true),
    }
}

/// Submits one run and waits for its result. Returns `None` at shutdown; the
/// run stays admitted and is delivered again after the restart.
async fn deliver(
    runtime: &NodeRuntime,
    connection: &Connection,
    due: &ScheduledRun,
    shutdown: &CancellationToken,
    failures: &mut Failures,
) -> Result<Option<RunOutcome>, NodeError> {
    let run = &due.run;
    let command_id = CommandId::from_uuid(run.id);
    // A run delivered again, after its result could not be recorded or the
    // node restarted, stays in the task it already executes in, whatever the
    // task resolution would pick now. One that already ended is not sent
    // again: the ledger's outcome stands, whatever the coordinator would say.
    let recorded = runtime.state.task_of_command(command_id).await?;
    if let Some(task_id) = recorded
        && let Some(outcome) = runtime.state.outcome(command_id).await?
    {
        return Ok(Some(finished(outcome, run, task_id)));
    }
    let task_id = match recorded {
        Some(task_id) => task_id,
        None => match task_for(runtime, connection, due, shutdown, failures).await? {
            Resolution::Task(task_id) => task_id,
            Resolution::Refused(reason) => return Ok(Some(refused(due, None, &reason))),
            Resolution::Shutdown => return Ok(None),
        },
    };
    loop {
        match connection
            .submit(task_id, command_id, submission(due))
            .await
        {
            Ok(()) => break,
            Err(error) if error.code() == Some(ErrorCode::NodeOffline) => {
                if !pause(shutdown, PAUSE).await {
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
        .map(|outcome| finished(outcome, run, task_id)))
}

/// The run's command input: its submission, quiet for a heartbeat's run.
fn submission(due: &ScheduledRun) -> CommandInput {
    CommandInput::Text {
        text: due.run.submission.clone(),
        context: None,
        author: Author::Principal,
        quiet: due.quiet,
    }
}

enum Resolution {
    Task(TaskId),
    Refused(String),
    Shutdown,
}

/// The task a run is submitted to: its conversation's, or else, unless it goes
/// only there, the automation's own, opened here when the run first needs it.
async fn task_for(
    runtime: &NodeRuntime,
    connection: &Connection,
    due: &ScheduledRun,
    shutdown: &CancellationToken,
    failures: &mut Failures,
) -> Result<Resolution, NodeError> {
    if let Some(session) = due.origin_session_id
        && let Some(task_id) = runtime.state.task_for_session(session).await?
    {
        return Ok(Resolution::Task(task_id));
    }
    if due.origin_required {
        return Ok(Resolution::Refused(
            "its conversation is not open here".to_owned(),
        ));
    }
    let agent_id = due.run.agent_id;
    let task_id = TaskId::from_uuid(due.run.automation_id);
    let target = agent_targets::target(agent_id);
    loop {
        let mut delay = PAUSE;
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
            [] => match runtime.host.agent_definition(agent_id).await {
                // The Host answered, so the heartbeat resumes on its next tick.
                Ok(Some(_)) => {
                    failures.cleared();
                }
                Ok(None) => {
                    return Ok(Resolution::Refused(format!(
                        "agent {agent_id} no longer exists"
                    )));
                }
                Err(error) => delay = failures.met(&error),
            },
            _ => {
                return Ok(Resolution::Refused(format!(
                    "more than one node advertises `{}`",
                    target.as_str()
                )));
            }
        }
        if !pause(shutdown, delay).await {
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

fn now_ms() -> Result<i64, NodeError> {
    Ok(TurnObservation::now()
        .map_err(|error| NodeError::Task(error.to_string()))?
        .unix_milliseconds())
}

/// Waits before a retry. Returns `false` when the node is shutting down.
async fn pause(shutdown: &CancellationToken, delay: Duration) -> bool {
    tokio::select! {
        () = shutdown.cancelled() => false,
        () = tokio::time::sleep(delay) => true,
    }
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
