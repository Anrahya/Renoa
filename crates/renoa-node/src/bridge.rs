use std::{collections::HashSet, sync::Arc, time::Duration};

use renoa_control::{DeviceCredentials, ErrorCode, TaskId};
use renoa_local::LocalHost;
use renoa_protocol::{TargetCard, TargetRef};
use thiserror::Error;
use tokio::{
    sync::{broadcast, watch},
    task::JoinSet,
    time::sleep,
};
use tokio_tungstenite::tungstenite::{Error as WebSocketError, client::IntoClientRequest};
use tokio_util::sync::CancellationToken;

use crate::{
    agent_link::{AgentLink, Peers},
    agent_targets,
    automations::{self, AutomationLink},
    backoff::{ReconnectBackoff, STABLE_CONNECTION},
    execution::{ExecutionTask, finish_execution, schedule_pending},
    host_control::HostControl,
    live::LiveFrame,
    node_log,
    node_store::{NodeStore, NodeStoreError, TargetBinding},
    session::{SessionEnd, serve_session},
    stop::RunningTurns,
};

/// How many live frames wait for the session loop before the oldest is
/// dropped. Live activity is never kept for later.
pub(crate) const LIVE_FRAME_CAPACITY: usize = 1024;
// A broadcast channel rounds its capacity up to a power of two, and the
// session bounds its drain of waiting frames by this number, so the two must
// agree.
const _: () = assert!(LIVE_FRAME_CAPACITY.is_power_of_two());

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
    drain: CancellationToken,
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
        let link = Arc::new(AgentLink::default());
        host.attach_agent_peers(Arc::new(Peers(Arc::clone(&link))));
        let host_control = HostControl::open(Arc::clone(&host))?;
        let (live, _) = broadcast::channel(LIVE_FRAME_CAPACITY);
        Ok(Self {
            endpoint,
            credentials,
            runtime: Arc::new(NodeRuntime {
                host,
                host_control,
                state,
                commits,
                link,
                live,
                turns: RunningTurns::default(),
            }),
            automations: None,
            drain: CancellationToken::new(),
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

    /// Drains the node once `drain` is cancelled: it takes no new command, so
    /// the coordinator keeps each one queued for the next node that connects,
    /// finishes every command it already took, and then [`Self::run`] returns.
    /// A deploy stops the node this way, so no turn is left unfinished under
    /// the runtime it replaces.
    #[must_use]
    pub fn with_drain(mut self, drain: CancellationToken) -> Self {
        self.drain = drain;
        self
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
            // A draining node that cannot reach the coordinator still stops,
            // after at most one reconnect delay.
            if shutdown.is_cancelled() || (self.drain.is_cancelled() && tasks.is_empty()) {
                return Ok(());
            }
            match serve_session(
                &self.endpoint,
                &self.credentials,
                Arc::clone(&self.runtime),
                shutdown,
                &self.drain,
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
    pub(crate) host_control: HostControl,
    pub(crate) state: NodeStore,
    pub(crate) commits: watch::Sender<u64>,
    /// How this node's agents reach the coordinator for other agents.
    pub(crate) link: Arc<AgentLink>,
    /// Ephemeral live activity for the session loop to forward.
    pub(crate) live: broadcast::Sender<LiveFrame>,
    /// The running turns a stop can cancel.
    pub(crate) turns: RunningTurns,
}

impl NodeRuntime {
    /// The Host agents this node offers for new tasks.
    pub(crate) async fn advertised_targets(&self) -> Result<Vec<TargetCard>, NodeError> {
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
