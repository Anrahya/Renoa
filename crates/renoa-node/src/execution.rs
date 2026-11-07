//! Running one admitted command on its Host session, and scheduling the
//! admitted commands of each task one at a time.

use std::{collections::HashSet, path::Path, sync::Arc};

use renoa_agent::ContentBlock;
use renoa_kernel::AgentId;
use renoa_local::{
    AgentSession, LocalHost, LocalHostError, LocalTurnOutcome, TurnObservation, UploadError,
    Uploads,
};
use renoa_protocol::{CommandEnvelope, CommandId, ExecutionEventKind, ExecutionTerminal};
use renoa_rcp_wire::TaskId;
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    bridge::{NodeError, NodeRuntime},
    control::{self, Refusal, Request},
    live::LiveEvents,
    node_log,
    node_store::ExecutionRecord,
    origin,
    projection::{closing_events, project_history},
    setup::SetupLinks,
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
        let (session, content) = match self.prepare(&record, agent_id, request).await {
            Ok(prepared) => prepared,
            Err(error) => {
                self.finish_failed(command_id, error).await?;
                return Ok(());
            }
        };
        self.state.append_turn_started(command_id).await?;
        self.signal_commit();
        let setup = Arc::new(SetupLinks::open(
            self.host.home(),
            &self.link,
            record.command.surface.as_str(),
            command_id.as_uuid(),
            cancellation.clone(),
        ));
        let events = Arc::new(
            LiveEvents::start(Arc::clone(&self), record.task_id, command_id, setup.clone()).await?,
        );
        let result = run_request(request, content, &record, &session, events, cancellation).await?;
        // A setup link that could not be delivered stopped the turn; its
        // reason is the command's outcome. The turn has settled (a tool runs
        // to its end even when cancelled), so every setup its calls saved is
        // saved by now.
        let setup_failure = match setup.finish().await {
            Some(stopped) => {
                stopped
                    .forget_setups(&self.host, record.binding.session_id, command_id.as_uuid())
                    .await
            }
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

    /// Opens the command's session and gathers what its turn receives. The
    /// workspace call also refuses an agent that no longer exists, which ends
    /// the execution as failed, with the returned reason, instead of stopping
    /// the node.
    async fn prepare(
        &self,
        record: &ExecutionRecord,
        agent_id: AgentId,
        request: Request,
    ) -> Result<(Arc<AgentSession>, Vec<ContentBlock>), String> {
        let workspace = self
            .host
            .agent_workspace(agent_id)
            .await
            .map_err(|error| error.to_string())?;
        let session = self
            .host
            .ensure_agent_session(
                agent_id,
                &record.binding.workspace,
                record.binding.session_id,
            )
            .await
            .map_err(|error| error.to_string())?;
        let content = match request {
            Request::Turn => turn_content(&self.host, &record.command, &workspace)
                .await
                .map_err(|error| {
                    format!("An attached file could not be given to the agent: {error}")
                })?,
            Request::Compact => Vec::new(),
        };
        Ok((session, content))
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

/// What the agent receives for a turn: the command's text, then each attached
/// file as the Host gives it ([`Uploads::attach`]). Text left blank beside an
/// attachment is not sent.
async fn turn_content(
    host: &LocalHost,
    command: &CommandEnvelope,
    workspace: &Path,
) -> Result<Vec<ContentBlock>, UploadError> {
    let attachments = command.input.attachments();
    let text = origin::prompt_text(command);
    let mut content = Vec::new();
    if attachments.is_empty() || !text.trim().is_empty() {
        content.push(ContentBlock::text(text));
    }
    let uploads = Uploads::open(host.home());
    for attachment in attachments {
        content.extend(
            uploads
                .attach(
                    command.principal_id.as_uuid(),
                    attachment.upload_id(),
                    attachment.name().to_owned(),
                    workspace.to_path_buf(),
                )
                .await?,
        );
    }
    Ok(content)
}

/// Runs one admitted command on its session. A turn observes when its text was
/// received and receives `content`; a compaction needs only the command's
/// identity. The inner result is the Host's outcome, the outer one a failure
/// of the node itself.
async fn run_request(
    request: Request,
    content: Vec<ContentBlock>,
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
                    content,
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
