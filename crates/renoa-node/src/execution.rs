//! Running one admitted command on its Host session, and scheduling the
//! admitted commands of each task one at a time.

use std::{collections::HashSet, sync::Arc};

use renoa_agent::ContentBlock;
use renoa_control::TaskId;
use renoa_kernel::AgentId;
use renoa_local::{AgentSession, LocalHostError, LocalTurnOutcome, TurnObservation};
use renoa_protocol::{CommandId, ExecutionEventKind, ExecutionTerminal};
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    bridge::{NodeError, NodeRuntime},
    control::{self, Refusal, Request},
    live::LiveEvents,
    node_log,
    node_store::ExecutionRecord,
    operator, origin,
    projection::{closing_events, project_history},
};

impl NodeRuntime {
    async fn execute(self: Arc<Self>, record: ExecutionRecord) -> Result<(), NodeError> {
        let command_id = record.command.command_id;
        let agent_id = AgentId::from_uuid(record.binding.agent_id);
        // Registered before the ledger is read, so a stop recorded from now on
        // cancels this token and an earlier one is read below.
        let cancellation = CancellationToken::new();
        let _running = self.turns.register(command_id, cancellation.clone());
        if self.state.stop_requested(command_id).await? {
            cancellation.cancel();
        }
        let request = match control::request(&record.command) {
            Ok(request) => request,
            // A command stopped before it started begins cancelled, refused
            // or not.
            Err(Refusal(_)) if cancellation.is_cancelled() => {
                self.finish_unstarted(command_id, closing_events(LocalTurnOutcome::Cancelled))
                    .await?;
                return Ok(());
            }
            Err(Refusal(reason)) => {
                self.finish_failed(command_id, reason.to_owned()).await?;
                return Ok(());
            }
        };
        node_log::event(
            "info",
            "execution_started",
            &serde_json::json!({
                "task_id": record.task_id,
                "command_id": command_id,
                "target": record.binding.target,
                "agent_id": record.binding.agent_id,
                "session_id": record.binding.session_id,
            }),
        );
        // The workspace call also refuses an agent that no longer exists, which
        // ends this execution as failed instead of stopping the node.
        let session = match self.host.agent_workspace(agent_id).await {
            Ok(_) => {
                self.host
                    .ensure_agent_session(
                        agent_id,
                        &record.binding.workspace,
                        record.binding.session_id,
                    )
                    .await
            }
            Err(error) => Err(error),
        };
        let session = match session {
            Ok(session) => session,
            Err(error) => {
                self.finish_failed(command_id, error.to_string()).await?;
                return Ok(());
            }
        };
        self.state.append_turn_started(command_id).await?;
        self.signal_commit();
        let (setup_events, setup) =
            operator::setup_sink(self.host.home(), command_id.as_uuid(), &cancellation);
        let events = Arc::new(
            LiveEvents::start(Arc::clone(&self), record.task_id, command_id, setup_events).await?,
        );
        let result = run_request(request, &record, &session, events, cancellation).await?;
        // A setup link that could not be delivered stopped the turn; its
        // reason is the command's outcome.
        let setup_failure = match &setup {
            Some(setup) => setup.take_error().await,
            None => None,
        };
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.finish_session_error(command_id, &session, &error)
                    .await?;
                return Ok(());
            }
        };
        let mut events = session_events(&session, command_id)?;
        match setup_failure {
            Some(error) => events.push(ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Failed { error },
            }),
            None => events.extend(closing_events(outcome)),
        }
        self.state.finish(command_id, events).await?;
        self.signal_commit();
        Ok(())
    }

    async fn finish_session_error(
        &self,
        command_id: CommandId,
        session: &AgentSession,
        error: &LocalHostError,
    ) -> Result<(), NodeError> {
        let mut events = session_events(session, command_id)?;
        events.push(ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Failed {
                error: error.to_string(),
            },
        });
        self.state.finish(command_id, events).await?;
        self.signal_commit();
        Ok(())
    }

    /// Ends a command as failed before any turn ran, so its journal holds only
    /// the terminal record.
    async fn finish_failed(&self, command_id: CommandId, error: String) -> Result<(), NodeError> {
        self.finish_unstarted(
            command_id,
            vec![ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Failed { error },
            }],
        )
        .await
    }

    /// Ends a command with `events` before any turn ran.
    async fn finish_unstarted(
        &self,
        command_id: CommandId,
        events: Vec<ExecutionEventKind>,
    ) -> Result<(), NodeError> {
        self.state.finish(command_id, events).await?;
        self.signal_commit();
        Ok(())
    }
}

/// Runs one admitted command on its session. A turn observes when its text was
/// received; a compaction needs only the command's identity. The inner result
/// is the Host's outcome, the outer one a failure of the node itself.
async fn run_request(
    request: Request,
    record: &ExecutionRecord,
    session: &AgentSession,
    events: Arc<LiveEvents>,
    cancellation: CancellationToken,
) -> Result<Result<LocalTurnOutcome, LocalHostError>, NodeError> {
    let command = &record.command;
    Ok(match request {
        Request::Turn => {
            let observation =
                TurnObservation::now().map_err(|error| NodeError::Task(error.to_string()))?;
            let observation = origin::observed(command, observation);
            session
                .execute_turn_observed_with_cancellation(
                    command.command_id.as_uuid(),
                    vec![ContentBlock::text(origin::prompt_text(command))],
                    observation,
                    events,
                    cancellation,
                    origin::speaker(command),
                )
                .await
        }
        Request::Compact => {
            session
                .execute_compaction_with_cancellation(
                    command.command_id.as_uuid(),
                    events,
                    cancellation,
                )
                .await
        }
    })
}

fn session_events(
    session: &AgentSession,
    command_id: CommandId,
) -> Result<Vec<ExecutionEventKind>, NodeError> {
    let history = session
        .history()
        .map_err(|error| NodeError::Task(format!("Host history projection failed: {error}")))?;
    let kernel_command = renoa_kernel::CommandId::from_uuid(command_id.as_uuid());
    project_history(
        history
            .into_iter()
            .filter(|entry| entry.command_id == kernel_command)
            .map(|entry| entry.message),
    )
}

pub(crate) struct ExecutionTask {
    task_id: TaskId,
    result: Result<(), NodeError>,
}

pub(crate) async fn schedule_pending(
    runtime: Arc<NodeRuntime>,
    tasks: &mut JoinSet<ExecutionTask>,
    running_tasks: &mut HashSet<TaskId>,
) -> Result<(), NodeError> {
    for record in runtime.state.load_unfinished().await? {
        if running_tasks.contains(&record.task_id) {
            continue;
        }
        let task_id = record.task_id;
        running_tasks.insert(task_id);
        let runtime = Arc::clone(&runtime);
        tasks.spawn(async move {
            let result = runtime.execute(record).await;
            ExecutionTask { task_id, result }
        });
    }
    Ok(())
}

pub(crate) fn finish_execution(
    completed: Result<ExecutionTask, JoinError>,
    running_tasks: &mut HashSet<TaskId>,
) -> Result<(), NodeError> {
    let completed = completed.map_err(|error| NodeError::Task(error.to_string()))?;
    running_tasks.remove(&completed.task_id);
    completed.result
}
