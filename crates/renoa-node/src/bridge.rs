use std::{collections::HashSet, sync::Arc, time::Duration};

use renoa_agent::ContentBlock;
use renoa_control::{DeviceCredentials, ErrorCode, TaskId};
use renoa_kernel::AgentId;
use renoa_local::{AgentSession, LocalHost, LocalHostError, Speaker, TurnObservation};
use renoa_protocol::{Author, CommandId, ExecutionEventKind, ExecutionTerminal, TargetRef};
use thiserror::Error;
use tokio::{
    sync::watch,
    task::{JoinError, JoinSet},
    time::sleep,
};
use tokio_tungstenite::tungstenite::{Error as WebSocketError, client::IntoClientRequest};
use tokio_util::sync::CancellationToken;

use crate::{
    agent_targets,
    automations::{self, AutomationLink},
    backoff::{ReconnectBackoff, STABLE_CONNECTION},
    live::LiveEvents,
    node_log,
    node_store::{ExecutionRecord, NodeStore, NodeStoreError, TargetBinding},
    operator,
    projection::{project_history, terminal_event},
    session::{SessionEnd, serve_session},
};

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("invalid Renoa node configuration: {0}")]
    Configuration(String),
    #[error("invalid coordinator endpoint: {0}")]
    Endpoint(#[source] WebSocketError),
    #[error("node storage failed: {0}")]
    Store(String),
    #[error("RCP message serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("coordinator rejected the node ({code:?}): {message}")]
    Rejected { code: ErrorCode, message: String },
    #[error("RCP protocol error: {0}")]
    Protocol(String),
    #[error("RCP transport disconnected: {0}")]
    Transport(String),
    #[error("execution task failed: {0}")]
    Task(String),
}

impl From<NodeStoreError> for NodeError {
    fn from(error: NodeStoreError) -> Self {
        Self::Store(error.to_string())
    }
}

/// A durable RCP execution node backed by Renoa's real local Host.
pub struct RenoaNode {
    endpoint: String,
    credentials: DeviceCredentials,
    runtime: Arc<NodeRuntime>,
    automations: Option<AutomationLink>,
}

impl RenoaNode {
    /// Opens the node ledger in the Host's installation root. Every Host agent
    /// becomes an advertised target, so there is no target configuration.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid endpoint, an unreadable ledger, or a
    /// recorded task binding that no longer names a Host agent workspace.
    pub fn open(
        endpoint: impl Into<String>,
        credentials: DeviceCredentials,
        host: Arc<LocalHost>,
    ) -> Result<Self, NodeError> {
        let endpoint = endpoint.into();
        endpoint
            .clone()
            .into_client_request()
            .map_err(NodeError::Endpoint)?;
        let state = NodeStore::open(host.home().node_database())?;
        state.validate_configured_targets(|binding| agent_targets::serves(host.home(), binding))?;
        let (commits, _) = watch::channel(0_u64);
        Ok(Self {
            endpoint,
            credentials,
            runtime: Arc::new(NodeRuntime {
                host,
                state,
                commits,
            }),
            automations: None,
        })
    }

    /// Runs the Host's automation schedule on this node. Each run is submitted
    /// through the surface enrolled with `credentials`, whose principal and
    /// surface name the run's command carries.
    ///
    /// # Errors
    ///
    /// Returns an error when another process owns the Host's schedule.
    pub fn with_automations(mut self, credentials: DeviceCredentials) -> Result<Self, NodeError> {
        let scheduler = self
            .runtime
            .host
            .automation_scheduler()
            .map_err(|error| NodeError::Configuration(error.to_string()))?;
        self.automations = Some(AutomationLink {
            endpoint: self.endpoint.clone(),
            credentials,
            scheduler,
        });
        Ok(self)
    }

    /// Runs durable Host work and, when configured, the Host's automation
    /// schedule, reconnecting both outbound coordinator links.
    ///
    /// # Errors
    ///
    /// Returns an error for authentication, protocol, local durability, or a
    /// failed execution task. Ordinary socket loss is retried.
    pub async fn run(mut self, shutdown: CancellationToken) -> Result<(), NodeError> {
        let mut tasks = JoinSet::new();
        let mut running_tasks = HashSet::new();
        schedule_pending(Arc::clone(&self.runtime), &mut tasks, &mut running_tasks).await?;
        let mut commits = self.runtime.commits.subscribe();
        // Boxed: the schedule's future is large and lives as long as the node.
        let schedule = self.automations.take().map(|link| {
            Box::pin(automations::run(
                Arc::clone(&self.runtime),
                link,
                shutdown.clone(),
            ))
        });
        let connections =
            self.run_connections(&shutdown, &mut commits, &mut tasks, &mut running_tasks);
        let result = match schedule {
            None => connections.await,
            Some(schedule) => tokio::select! {
                result = connections => result,
                result = schedule => result,
            },
        };

        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        result
    }

    async fn run_connections(
        &self,
        shutdown: &CancellationToken,
        commits: &mut watch::Receiver<u64>,
        tasks: &mut JoinSet<ExecutionTask>,
        running_tasks: &mut HashSet<TaskId>,
    ) -> Result<(), NodeError> {
        let mut backoff = ReconnectBackoff::new();
        loop {
            if shutdown.is_cancelled() {
                return Ok(());
            }
            match serve_session(
                &self.endpoint,
                &self.credentials,
                Arc::clone(&self.runtime),
                shutdown,
                commits,
                tasks,
                running_tasks,
            )
            .await?
            {
                SessionEnd::Shutdown => return Ok(()),
                SessionEnd::Disconnected {
                    reason,
                    connected_for,
                } => {
                    let stable = connected_for.is_some_and(|elapsed| elapsed >= STABLE_CONNECTION);
                    let delay = backoff.next_delay(stable);
                    node_log::event(
                        "warn",
                        "coordinator_disconnected",
                        &serde_json::json!({
                            "reason": reason,
                            "connected_ms": connected_for.and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok()),
                            "retry_ms": u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        }),
                    );
                    if !wait_to_reconnect(
                        shutdown,
                        Arc::clone(&self.runtime),
                        tasks,
                        running_tasks,
                        delay,
                    )
                    .await?
                    {
                        return Ok(());
                    }
                }
            }
        }
    }
}

pub(crate) struct NodeRuntime {
    pub(crate) host: Arc<LocalHost>,
    pub(crate) state: NodeStore,
    pub(crate) commits: watch::Sender<u64>,
}

impl NodeRuntime {
    /// The Host agents this node offers for new tasks.
    pub(crate) async fn advertised_targets(&self) -> Result<Vec<TargetRef>, NodeError> {
        agent_targets::advertised(&self.host).await
    }

    pub(crate) fn proposed_binding(&self, target: &TargetRef) -> Result<TargetBinding, NodeError> {
        agent_targets::proposed_binding(self.host.home(), target)
    }

    pub(crate) fn signal_commit(&self) {
        self.commits.send_modify(|version| {
            *version = version.wrapping_add(1);
        });
    }

    async fn execute(self: Arc<Self>, record: ExecutionRecord) -> Result<(), NodeError> {
        let command_id = record.command.command_id;
        let agent_id = AgentId::from_uuid(record.binding.agent_id);
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
                self.finish_host_error(command_id, &error).await?;
                return Ok(());
            }
        };
        self.state.append_turn_started(command_id).await?;
        self.signal_commit();
        let cancellation = CancellationToken::new();
        let (setup_events, setup) =
            operator::setup_sink(self.host.home(), command_id.as_uuid(), &cancellation);
        let events =
            Arc::new(LiveEvents::start(Arc::clone(&self), command_id, setup_events).await?);
        let mut observation =
            TurnObservation::now().map_err(|error| NodeError::Task(error.to_string()))?;
        if let Some(context) = record.command.input.context() {
            observation = observation.with_surface_context(context);
        }
        let result = session
            .execute_turn_observed_with_cancellation(
                command_id.as_uuid(),
                vec![ContentBlock::text(record.command.input.text())],
                observation,
                events,
                cancellation,
                match record.command.input.author() {
                    Author::Principal => Speaker::Principal(record.command.principal_id.as_uuid()),
                    Author::Guest => Speaker::Guest,
                },
            )
            .await;
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
        events.push(match setup_failure {
            Some(error) => ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Failed { error },
            },
            None => terminal_event(outcome),
        });
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

    async fn finish_host_error(
        &self,
        command_id: CommandId,
        error: &LocalHostError,
    ) -> Result<(), NodeError> {
        self.state
            .finish(
                command_id,
                vec![ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Failed {
                        error: error.to_string(),
                    },
                }],
            )
            .await?;
        self.signal_commit();
        Ok(())
    }
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

async fn wait_to_reconnect(
    shutdown: &CancellationToken,
    runtime: Arc<NodeRuntime>,
    tasks: &mut JoinSet<ExecutionTask>,
    running_tasks: &mut HashSet<TaskId>,
    reconnect_delay: Duration,
) -> Result<bool, NodeError> {
    let delay = sleep(reconnect_delay);
    tokio::pin!(delay);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(false),
            () = &mut delay => return Ok(true),
            completed = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(completed) = completed {
                    finish_execution(completed, running_tasks)?;
                    schedule_pending(Arc::clone(&runtime), tasks, running_tasks).await?;
                }
            }
        }
    }
}
