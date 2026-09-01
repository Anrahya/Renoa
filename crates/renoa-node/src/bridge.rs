use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::Arc,
    time::Duration,
};

use renoa_agent::ContentBlock;
use renoa_control::{DeviceCredentials, ErrorCode, TaskId};
use renoa_local::{AgentProfileId, AgentSession, LocalHost, LocalHostError};
use renoa_protocol::{CommandId, ExecutionEventKind, ExecutionTerminal, TargetRef};
use thiserror::Error;
use tokio::{
    sync::watch,
    task::{JoinError, JoinSet},
    time::sleep,
};
use tokio_tungstenite::tungstenite::{Error as WebSocketError, client::IntoClientRequest};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    node_store::{ExecutionRecord, NodeStore, NodeStoreError, TargetBinding},
    projection::{NoopEvents, project_history, terminal_event},
    session::{SessionEnd, serve_session},
};

const RECONNECT_DELAY: Duration = Duration::from_millis(100);

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
    #[error("execution task failed: {0}")]
    Task(String),
}

impl From<NodeStoreError> for NodeError {
    fn from(error: NodeStoreError) -> Self {
        Self::Store(error.to_string())
    }
}

/// One coordinator target resolved to an exact Host profile, session, and workspace.
#[derive(Clone, Debug)]
pub struct HostTarget {
    binding: TargetBinding,
    profile_id: AgentProfileId,
}

impl HostTarget {
    /// Creates one stable RCP-to-Host binding.
    ///
    /// # Errors
    ///
    /// Returns an error when the target is empty or the workspace is not an
    /// existing absolute directory.
    pub fn new(
        target: &TargetRef,
        profile_id: AgentProfileId,
        session_id: Uuid,
        workspace: impl AsRef<Path>,
    ) -> Result<Self, NodeError> {
        if target.as_str().is_empty() {
            return Err(NodeError::Configuration(
                "Host target identity must not be empty".to_owned(),
            ));
        }
        let workspace = std::fs::canonicalize(workspace.as_ref()).map_err(|error| {
            NodeError::Configuration(format!("Host target workspace cannot be resolved: {error}"))
        })?;
        if !workspace.is_dir() {
            return Err(NodeError::Configuration(
                "Host target workspace must be a directory".to_owned(),
            ));
        }
        if workspace.to_str().is_none() {
            return Err(NodeError::Configuration(
                "Host target workspace must be valid UTF-8".to_owned(),
            ));
        }
        Ok(Self {
            binding: TargetBinding {
                target: target.as_str().to_owned(),
                profile_id: profile_id.as_str().to_owned(),
                session_id,
                workspace,
            },
            profile_id,
        })
    }
}

/// A durable RCP execution node backed by Renoa's real local Host.
pub struct RenoaNode {
    endpoint: String,
    credentials: DeviceCredentials,
    runtime: Arc<NodeRuntime>,
}

impl RenoaNode {
    /// Opens the node ledger and validates every configured Host target.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid endpoint, target configuration, or
    /// durable binding mismatch.
    pub fn open(
        endpoint: impl Into<String>,
        credentials: DeviceCredentials,
        ledger_path: impl AsRef<Path>,
        host: Arc<LocalHost>,
        targets: Vec<HostTarget>,
    ) -> Result<Self, NodeError> {
        let endpoint = endpoint.into();
        endpoint
            .clone()
            .into_client_request()
            .map_err(NodeError::Endpoint)?;
        let targets = validate_targets(targets)?;
        let state = NodeStore::open(ledger_path)?;
        let durable_targets = targets
            .values()
            .map(|target| target.binding.clone())
            .collect::<Vec<_>>();
        state.validate_configured_targets(&durable_targets)?;
        let (commits, _) = watch::channel(0_u64);
        Ok(Self {
            endpoint,
            credentials,
            runtime: Arc::new(NodeRuntime {
                host,
                targets,
                state,
                commits,
            }),
        })
    }

    /// Runs durable Host work and reconnects its outbound coordinator session.
    ///
    /// # Errors
    ///
    /// Returns an error for authentication, protocol, local durability, or a
    /// failed execution task. Ordinary socket loss is retried.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), NodeError> {
        let mut tasks = JoinSet::new();
        let mut running_tasks = HashSet::new();
        schedule_pending(Arc::clone(&self.runtime), &mut tasks, &mut running_tasks).await?;
        let mut commits = self.runtime.commits.subscribe();
        let result = self
            .run_connections(&shutdown, &mut commits, &mut tasks, &mut running_tasks)
            .await;

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
                SessionEnd::Disconnected => {}
            }
            if !wait_to_reconnect(shutdown, Arc::clone(&self.runtime), tasks, running_tasks).await?
            {
                return Ok(());
            }
        }
    }
}

pub(crate) struct NodeRuntime {
    pub(crate) host: Arc<LocalHost>,
    targets: BTreeMap<String, HostTarget>,
    pub(crate) state: NodeStore,
    pub(crate) commits: watch::Sender<u64>,
}

impl NodeRuntime {
    pub(crate) fn binding_for(&self, target: &TargetRef) -> Result<TargetBinding, NodeError> {
        self.targets
            .get(target.as_str())
            .map(|target| target.binding.clone())
            .ok_or_else(|| {
                NodeError::Protocol(format!(
                    "coordinator requested unconfigured target `{}`",
                    target.as_str()
                ))
            })
    }

    fn target_for(&self, binding: &TargetBinding) -> Result<&HostTarget, NodeError> {
        self.targets
            .get(&binding.target)
            .filter(|target| target.binding == *binding)
            .ok_or_else(|| {
                NodeError::Configuration(format!(
                    "durable target `{}` no longer matches node configuration",
                    binding.target
                ))
            })
    }

    pub(crate) fn signal_commit(&self) {
        self.commits.send_modify(|version| {
            *version = version.wrapping_add(1);
        });
    }

    async fn execute(self: Arc<Self>, record: ExecutionRecord) -> Result<(), NodeError> {
        let command_id = record.command.command_id;
        let target = self.target_for(&record.binding)?.clone();
        let session = match self
            .host
            .ensure_session(
                &target.profile_id,
                &target.binding.workspace,
                target.binding.session_id,
            )
            .await
        {
            Ok(session) => session,
            Err(error) => {
                self.finish_host_error(command_id, &error).await?;
                return Ok(());
            }
        };
        self.state.append_turn_started(command_id).await?;
        self.signal_commit();
        let result = session
            .execute_turn(
                command_id.as_uuid(),
                vec![ContentBlock::text(record.command.input.text())],
                Arc::new(NoopEvents),
            )
            .await;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.finish_session_error(command_id, &session, &error)
                    .await?;
                return Ok(());
            }
        };
        let mut events = session_events(&session, command_id)?;
        events.push(terminal_event(outcome));
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
) -> Result<bool, NodeError> {
    let delay = sleep(RECONNECT_DELAY);
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

fn validate_targets(targets: Vec<HostTarget>) -> Result<BTreeMap<String, HostTarget>, NodeError> {
    if targets.is_empty() {
        return Err(NodeError::Configuration(
            "at least one Host target must be configured".to_owned(),
        ));
    }
    let mut by_name = BTreeMap::new();
    let mut sessions = HashSet::new();
    for target in targets {
        if !sessions.insert(target.binding.session_id) {
            return Err(NodeError::Configuration(format!(
                "Host session {} is configured for more than one target",
                target.binding.session_id
            )));
        }
        let name = target.binding.target.clone();
        if by_name.insert(name.clone(), target).is_some() {
            return Err(NodeError::Configuration(format!(
                "Host target `{name}` is configured more than once"
            )));
        }
    }
    Ok(by_name)
}
