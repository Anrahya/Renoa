use std::{collections::HashSet, path::Path, sync::Arc, time::Duration};

use renoa_control::{DeviceCredentials, ErrorCode, TaskId};
use renoa_core::{
    CapabilityHost, CommandEnvelope, CommandId, ModelDriver, ResolvedAgent, RunStatus, RunStore,
    StoreError, TerminalState,
};
use renoa_runtime::{Engine, EngineConfig, EngineError};
use renoa_store_sqlite::SqliteRunStore;
use thiserror::Error;
use tokio::{
    sync::watch,
    task::{JoinError, JoinSet},
    time::sleep,
};
use tokio_tungstenite::tungstenite::{Error as WebSocketError, client::IntoClientRequest};
use tokio_util::sync::CancellationToken;

use crate::{
    live_store::LiveRunStore,
    node_store::NodeStore,
    session::{SessionEnd, serve_session},
};

const RECONNECT_DELAY: Duration = Duration::from_millis(100);
const INTERRUPTED_RUN: &str = "execution interrupted by node restart";

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("invalid coordinator endpoint: {0}")]
    Endpoint(#[source] WebSocketError),
    #[error("node storage failed: {0}")]
    Store(#[from] StoreError),
    #[error("RCP message serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("coordinator rejected the node ({code:?}): {message}")]
    Rejected { code: ErrorCode, message: String },
    #[error("RCP protocol error: {0}")]
    Protocol(String),
    #[error("execution {command_id} failed at the node boundary: {message}")]
    Execution {
        command_id: CommandId,
        message: String,
    },
    #[error("execution task failed: {0}")]
    Task(String),
}

/// A concrete RCP execution node backed by Renoa's reference engine.
pub struct RenoaNode {
    endpoint: String,
    credentials: DeviceCredentials,
    runtime: Arc<NodeRuntime>,
}

impl RenoaNode {
    /// Opens the durable node ledger and validates the coordinator endpoint.
    ///
    /// # Errors
    ///
    /// Returns `NodeError` when the endpoint is invalid or local `SQLite` state
    /// cannot be initialized.
    pub fn open(
        endpoint: impl Into<String>,
        credentials: DeviceCredentials,
        ledger_path: impl AsRef<Path>,
        agent: ResolvedAgent,
        model: Arc<dyn ModelDriver>,
        capabilities: Arc<dyn CapabilityHost>,
        config: EngineConfig,
    ) -> Result<Self, NodeError> {
        let endpoint = endpoint.into();
        endpoint
            .clone()
            .into_client_request()
            .map_err(NodeError::Endpoint)?;
        let run_store = Arc::new(SqliteRunStore::open(&ledger_path)?);
        let state = NodeStore::open(ledger_path)?;
        let (commits, _) = watch::channel(0_u64);
        Ok(Self {
            endpoint,
            credentials,
            runtime: Arc::new(NodeRuntime {
                agent,
                model,
                capabilities,
                run_store,
                state,
                commits,
                config,
            }),
        })
    }

    /// Connects outward to the coordinator, executes delivered commands, and
    /// reconnects after transport loss until `shutdown` is cancelled.
    ///
    /// # Errors
    ///
    /// Returns `NodeError` for authentication, protocol, local durability, or
    /// execution-boundary failures. Ordinary socket loss is retried.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), NodeError> {
        self.runtime.recover_interrupted().await?;
        let execution_shutdown = CancellationToken::new();
        let mut tasks = JoinSet::new();
        let mut running = HashSet::new();
        let mut commits = self.runtime.commits.subscribe();
        let result = self
            .run_connections(
                &shutdown,
                &execution_shutdown,
                &mut commits,
                &mut tasks,
                &mut running,
            )
            .await;

        execution_shutdown.cancel();
        while let Some(completed) = tasks.join_next().await {
            if result.is_ok() {
                finish_execution(completed, &mut running)?;
            }
        }
        result
    }

    async fn run_connections(
        &self,
        shutdown: &CancellationToken,
        execution_shutdown: &CancellationToken,
        commits: &mut watch::Receiver<u64>,
        tasks: &mut JoinSet<ExecutionTask>,
        running: &mut HashSet<CommandId>,
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
                execution_shutdown,
                commits,
                tasks,
                running,
            )
            .await?
            {
                SessionEnd::Shutdown => return Ok(()),
                SessionEnd::Disconnected => {}
            }
            if !wait_to_reconnect(shutdown, tasks, running).await? {
                return Ok(());
            }
        }
    }
}

pub(crate) struct NodeRuntime {
    pub(crate) agent: ResolvedAgent,
    pub(crate) model: Arc<dyn ModelDriver>,
    pub(crate) capabilities: Arc<dyn CapabilityHost>,
    pub(crate) run_store: Arc<SqliteRunStore>,
    pub(crate) state: NodeStore,
    pub(crate) commits: watch::Sender<u64>,
    pub(crate) config: EngineConfig,
}

impl NodeRuntime {
    async fn recover_interrupted(&self) -> Result<(), NodeError> {
        for execution in self.state.load_all().await? {
            let transcript = self.run_store.load_transcript(execution.run_id).await?;
            if transcript.run.status == RunStatus::Open {
                self.run_store
                    .finish_run(
                        execution.run_id,
                        TerminalState::Failed {
                            error: INTERRUPTED_RUN.to_owned(),
                        },
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn execute(
        self: Arc<Self>,
        task_id: TaskId,
        command: CommandEnvelope,
        cancellation: CancellationToken,
    ) -> Result<(), NodeError> {
        let command_id = command.command_id;
        let live_store = Arc::new(LiveRunStore::new(
            Arc::clone(&self.run_store),
            self.state.clone(),
            task_id,
            command_id,
            self.commits.clone(),
        ));
        let engine_store: Arc<dyn RunStore> = live_store.clone();
        let engine = Engine::new(
            Arc::clone(&self.model),
            Arc::clone(&self.capabilities),
            engine_store,
            self.config,
        );
        match engine.run(command, self.agent.clone(), cancellation).await {
            Err(EngineError::CommandAlreadyAdmitted(run_id)) => {
                let transcript = live_store.load_transcript(run_id).await?;
                if transcript.run.status == RunStatus::Open {
                    live_store
                        .finish_run(
                            run_id,
                            TerminalState::Failed {
                                error: INTERRUPTED_RUN.to_owned(),
                            },
                        )
                        .await?;
                }
                Ok(())
            }
            Err(EngineError::CommandConflict(_)) => Err(NodeError::Execution {
                command_id,
                message: "stable command identity conflicts with the local ledger".to_owned(),
            }),
            Err(EngineError::Store(error)) => Err(NodeError::Store(error)),
            Ok(_)
            | Err(
                EngineError::Cancelled
                | EngineError::CapabilityBatchTooLarge { .. }
                | EngineError::CapabilityTask(_)
                | EngineError::Model(_)
                | EngineError::RoundLimit(_),
            ) => Ok(()),
        }
    }
}

pub(crate) struct ExecutionTask {
    command_id: CommandId,
    result: Result<(), NodeError>,
}

pub(crate) fn start_execution(
    runtime: Arc<NodeRuntime>,
    task_id: TaskId,
    command: CommandEnvelope,
    cancellation: CancellationToken,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<CommandId>,
) {
    let command_id = command.command_id;
    if !running.insert(command_id) {
        return;
    }
    tasks.spawn(async move {
        let result = runtime.execute(task_id, command, cancellation).await;
        ExecutionTask { command_id, result }
    });
}

pub(crate) fn finish_execution(
    completed: Result<ExecutionTask, JoinError>,
    running: &mut HashSet<CommandId>,
) -> Result<(), NodeError> {
    let completed = completed.map_err(|error| NodeError::Task(error.to_string()))?;
    running.remove(&completed.command_id);
    completed.result
}

async fn wait_to_reconnect(
    shutdown: &CancellationToken,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<CommandId>,
) -> Result<bool, NodeError> {
    let delay = sleep(RECONNECT_DELAY);
    tokio::pin!(delay);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(false),
            () = &mut delay => return Ok(true),
            completed = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(completed) = completed {
                    finish_execution(completed, running)?;
                }
            }
        }
    }
}
