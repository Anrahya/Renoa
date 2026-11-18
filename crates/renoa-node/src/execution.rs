//! Running one admitted command on its Host session, and scheduling the
//! admitted commands of each task one at a time.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

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
    steering::{RunningTasks, SteerInbox},
};

impl NodeRuntime {
    async fn execute(
        self: Arc<Self>,
        record: ExecutionRecord,
        steers: SteerInbox,
    ) -> Result<(), NodeError> {
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
        let Some(Prepared {
            session,
            workspace,
            synthetic_tools,
            content,
        }) = self.open_turn(&record, agent_id, request).await?
        else {
            return Ok(());
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
            LiveEvents::start(
                Arc::clone(&self),
                record.task_id,
                command_id,
                synthetic_tools.clone(),
                setup.clone(),
            )
            .await?,
        );
        let run = run_request(
            request,
            content,
            &record,
            &session,
            Arc::clone(&events),
            cancellation,
        );
        let offers = self.offer_steers(command_id, &session, &workspace, steers);
        let result = while_offering(run, offers).await?;
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
                self.finish_session_error(command_id, &session, &synthetic_tools, &error)
                    .await?;
                return Ok(());
            }
        };
        let mut events = session_events(&session, command_id, &synthetic_tools)?;
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

    /// Opens the turn of `record`, or ends the command before any turn runs
    /// and returns `None`: as failed when it cannot run, or as completed when
    /// the turn before it took it in as a steer. Its message is then part of
    /// that turn, so it has no reply of its own.
    async fn open_turn(
        &self,
        record: &ExecutionRecord,
        agent_id: AgentId,
        request: Request,
    ) -> Result<Option<Prepared>, NodeError> {
        let command_id = record.command.command_id;
        let mut prepared = match self.prepare(record, agent_id).await {
            Ok(prepared) => prepared,
            Err(error) => {
                self.finish_failed(command_id, error).await?;
                return Ok(None);
            }
        };
        if record.command.input.steer()
            && prepared
                .session
                .steered(command_id.as_uuid())
                .map_err(|error| NodeError::Task(format!("Host steer state failed: {error}")))?
        {
            let completed = ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            };
            self.finish_unstarted(command_id, vec![completed]).await?;
            return Ok(None);
        }
        if request == Request::Turn {
            match turn_content(&self.host, &record.command, &prepared.workspace).await {
                Ok(content) => prepared.content = content,
                Err(error) => {
                    let error =
                        format!("An attached file could not be given to the agent: {error}");
                    self.finish_failed(command_id, error).await?;
                    return Ok(None);
                }
            }
        }
        Ok(Some(prepared))
    }

    /// Opens the command's session and finds its agent's files and the tools
    /// whose calls show as activity. The workspace call also refuses an agent that no longer exists, which ends
    /// the execution as failed, with the returned reason, instead of stopping
    /// the node.
    async fn prepare(
        &self,
        record: &ExecutionRecord,
        agent_id: AgentId,
    ) -> Result<Prepared, String> {
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
        let synthetic_tools = session
            .synthetic_activity_tools()
            .await
            .map_err(|error| error.to_string())?;
        Ok(Prepared {
            session,
            workspace,
            synthetic_tools,
            content: Vec::new(),
        })
    }

    async fn finish_session_error(
        &self,
        command_id: CommandId,
        session: &AgentSession,
        synthetic_activity_tools: &HashSet<String>,
        error: &LocalHostError,
    ) -> Result<(), NodeError> {
        let mut events = session_events(session, command_id, synthetic_activity_tools)?;
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

/// The session a command runs on, where its agent's files are, the tools
/// whose calls show as activity, and what its turn receives (nothing for a
/// compaction).
struct Prepared {
    session: Arc<AgentSession>,
    workspace: PathBuf,
    synthetic_tools: HashSet<String>,
    content: Vec<ContentBlock>,
}

/// What the agent receives for a turn: the command's text, then each attached
/// file as the Host gives it ([`Uploads::attach`]). Text left blank beside an
/// attachment is not sent.
pub(crate) async fn turn_content(
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

/// Awaits `run` while `offers` hands steers to its turn. Offers end when the
/// run does; an offer cut short is settled by the steer's own place in the queue.
async fn while_offering<T>(run: impl Future<Output = T>, offers: impl Future<Output = ()>) -> T {
    tokio::pin!(run, offers);
    tokio::select! {
        biased;
        result = &mut run => result,
        () = &mut offers => run.await,
    }
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
    synthetic_activity_tools: &HashSet<String>,
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
        synthetic_activity_tools,
    )
}

pub(crate) struct ExecutionTask {
    task_id: TaskId,
    result: Result<(), NodeError>,
}

/// Starts the first unfinished command of each task with none running. A
/// command queued behind a running one may steer it (see [`crate::steering`]).
pub(crate) async fn schedule_pending(
    runtime: Arc<NodeRuntime>,
    tasks: &mut JoinSet<ExecutionTask>,
    running_tasks: &mut RunningTasks,
) -> Result<(), NodeError> {
    let mut held = HashSet::new();
    for record in runtime.state.load_unfinished().await? {
        if running_tasks.queue_behind(&record, &mut held) {
            continue;
        }
        let task_id = record.task_id;
        let steers = running_tasks.start(&record);
        let runtime = Arc::clone(&runtime);
        tasks.spawn(async move {
            let result = Box::pin(runtime.execute(record, steers)).await;
            ExecutionTask { task_id, result }
        });
    }
    Ok(())
}

pub(crate) fn finish_execution(
    completed: Result<ExecutionTask, JoinError>,
    running_tasks: &mut RunningTasks,
) -> Result<(), NodeError> {
    let completed = completed.map_err(|error| NodeError::Task(error.to_string()))?;
    running_tasks.finish(completed.task_id);
    completed.result
}
