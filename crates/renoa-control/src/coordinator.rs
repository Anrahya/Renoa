use std::{collections::HashMap, path::Path, sync::Arc, time::SystemTime};

use axum::{Router, routing::get};
use renoa_protocol::{CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef};
use thiserror::Error;
use tokio::{
    net::TcpListener,
    sync::{Mutex, Semaphore, broadcast, mpsc},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    DeviceId, ErrorCode, NodeId, PasskeyBootstrapToken, PeerIdentity, ServerMessage, TaskEvent,
    TaskId,
    browser_identity::BrowserIdentity,
    browser_identity_http,
    connection::upgrade_connection,
    operations::SurfaceOperation,
    store::{CommandAdmission, ControlStore},
    wire::{publish_task_event, send_control_error, send_error, task_sender},
};

const MAX_CONCURRENT_CONNECTIONS: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct TaskSpec {
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
    pub node_id: NodeId,
    pub target: TargetRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlErrorKind {
    Authentication,
    Capacity,
    Conflict,
    Invalid,
    NotFound,
    Store,
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct ControlError {
    kind: ControlErrorKind,
    message: String,
}

impl ControlError {
    pub(crate) fn authentication_failed() -> Self {
        Self::new(ControlErrorKind::Authentication, "authentication failed")
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new(ControlErrorKind::Conflict, message)
    }

    pub(crate) fn capacity(message: impl Into<String>) -> Self {
        Self::new(ControlErrorKind::Capacity, message)
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ControlErrorKind::Invalid, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(ControlErrorKind::NotFound, message)
    }

    pub(crate) fn store(message: impl Into<String>) -> Self {
        Self::new(ControlErrorKind::Store, message)
    }

    fn new(kind: ControlErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) const fn kind(&self) -> ControlErrorKind {
        self.kind
    }

    pub(crate) fn protocol_code(&self) -> ErrorCode {
        match self.kind {
            ControlErrorKind::Authentication => ErrorCode::AuthenticationFailed,
            ControlErrorKind::Capacity | ControlErrorKind::Store => ErrorCode::Internal,
            ControlErrorKind::Conflict => ErrorCode::Conflict,
            ControlErrorKind::Invalid => ErrorCode::InvalidMessage,
            ControlErrorKind::NotFound => ErrorCode::NotFound,
        }
    }
}

#[derive(Clone)]
pub struct Coordinator {
    state: Arc<CoordinatorState>,
}

pub(crate) struct CoordinatorState {
    pub(crate) browser_identity: Option<BrowserIdentity>,
    pub(crate) connection_slots: Arc<Semaphore>,
    pub(crate) connection_lifecycle: Mutex<()>,
    pub(crate) store: ControlStore,
    pub(crate) nodes: Mutex<HashMap<NodeId, NodeConnection>>,
    pub(crate) sessions: Mutex<HashMap<DeviceId, HashMap<Uuid, CancellationToken>>>,
    pub(crate) task_senders: Mutex<HashMap<TaskId, broadcast::Sender<TaskEvent>>>,
}

#[derive(Clone)]
pub(crate) struct NodeConnection {
    pub(crate) connection_id: Uuid,
    pub(crate) device_id: DeviceId,
    pub(crate) outgoing: mpsc::Sender<ServerMessage>,
}

impl Coordinator {
    /// Opens the coordinator's durable task journal.
    ///
    /// # Errors
    ///
    /// Returns an error when the `SQLite` journal cannot be opened or initialized.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ControlError> {
        Self::open_inner(path, None)
    }

    /// Opens the coordinator with browser passkey authentication at one exact HTTPS origin.
    ///
    /// # Errors
    ///
    /// Returns an error when the database or passkey relying-party configuration is invalid.
    pub fn open_with_passkeys(
        path: impl AsRef<Path>,
        rp_id: &str,
        rp_origin: &str,
    ) -> Result<Self, ControlError> {
        Self::open_inner(path, Some(BrowserIdentity::new(rp_id, rp_origin)?))
    }

    fn open_inner(
        path: impl AsRef<Path>,
        browser_identity: Option<BrowserIdentity>,
    ) -> Result<Self, ControlError> {
        Ok(Self {
            state: Arc::new(CoordinatorState {
                browser_identity,
                connection_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS)),
                connection_lifecycle: Mutex::new(()),
                store: ControlStore::open(path)?,
                nodes: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                task_senders: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Creates one durable task and its execution binding.
    ///
    /// # Errors
    ///
    /// Returns an error when the identity already exists or storage fails.
    pub async fn create_task(&self, task: TaskSpec) -> Result<(), ControlError> {
        self.state.store.create_task(task).await
    }

    /// Creates a single-use enrollment bound to one server-selected peer identity.
    ///
    /// # Errors
    ///
    /// Returns an error when the enrollment cannot be persisted.
    pub async fn create_enrollment(
        &self,
        peer: PeerIdentity,
        expires_at: SystemTime,
    ) -> Result<crate::EnrollmentToken, ControlError> {
        self.state.store.create_enrollment(peer, expires_at).await
    }

    /// Creates a local, single-use bootstrap for registering a passkey to one principal.
    ///
    /// # Errors
    ///
    /// Returns an error when the bootstrap cannot be persisted.
    pub async fn create_passkey_bootstrap(
        &self,
        principal_id: PrincipalId,
        expires_at: SystemTime,
    ) -> Result<PasskeyBootstrapToken, ControlError> {
        self.state
            .store
            .create_passkey_bootstrap(principal_id, expires_at)
            .await
    }

    /// Revokes a device credential and terminates its active connections.
    ///
    /// # Errors
    ///
    /// Returns an error when the device does not exist or revocation cannot be persisted.
    pub async fn revoke_device(&self, device_id: DeviceId) -> Result<(), ControlError> {
        self.state.store.revoke_device(device_id).await?;
        let _lifecycle = self.state.connection_lifecycle.lock().await;
        let sessions = self.state.sessions.lock().await.remove(&device_id);
        for session in sessions.into_iter().flatten().map(|(_, session)| session) {
            session.cancel();
        }
        self.state
            .nodes
            .lock()
            .await
            .retain(|_, node| node.device_id != device_id);
        Ok(())
    }

    /// Serves the authenticated protocol over a plaintext loopback listener.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-loopback listener or when the HTTP server fails.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: CancellationToken,
    ) -> Result<(), ControlError> {
        let address = listener
            .local_addr()
            .map_err(|error| ControlError::store(format!("listener address failed: {error}")))?;
        if !address.ip().is_loopback() {
            return Err(ControlError::invalid(
                "the plaintext coordinator is loopback-only",
            ));
        }
        let mut app = Router::new().route("/connect", get(upgrade_connection));
        if self.state.browser_identity.is_some() {
            app = app.merge(browser_identity_http::routes());
        }
        let app = app.with_state(Arc::clone(&self.state));
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .map_err(|error| ControlError::store(format!("coordinator server failed: {error}")))
    }
}

pub(crate) async fn handle_surface_operation(
    state: Arc<CoordinatorState>,
    outgoing: &mpsc::Sender<ServerMessage>,
    connection_cancelled: &CancellationToken,
    principal_id: PrincipalId,
    surface: SurfaceRef,
    request_id: u64,
    operation: SurfaceOperation,
) {
    match operation {
        SurfaceOperation::ListTasks => match state.store.list_tasks(principal_id).await {
            Ok(tasks) => {
                let _ = outgoing
                    .send(ServerMessage::TaskList { request_id, tasks })
                    .await;
            }
            Err(error) => send_control_error(outgoing, Some(request_id), &error).await,
        },
        SurfaceOperation::Attach {
            task_id,
            after_sequence,
        } => {
            if let Err(error) = attach_surface(
                state,
                outgoing.clone(),
                connection_cancelled.child_token(),
                request_id,
                task_id,
                after_sequence,
                principal_id,
            )
            .await
            {
                send_control_error(outgoing, Some(request_id), &error).await;
            }
        }
        SurfaceOperation::Submit {
            task_id,
            command_id,
            input,
        } => {
            let result = submit_command(
                &state,
                outgoing,
                request_id,
                task_id,
                command_id,
                input,
                principal_id,
                surface,
            )
            .await;
            if let Err(error) = result {
                send_control_error(outgoing, Some(request_id), &error).await;
            }
        }
    }
}

async fn attach_surface(
    state: Arc<CoordinatorState>,
    outgoing: mpsc::Sender<ServerMessage>,
    cancelled: CancellationToken,
    request_id: u64,
    task_id: TaskId,
    after_sequence: Option<u64>,
    principal_id: PrincipalId,
) -> Result<(), ControlError> {
    // Reject unknown tasks before allocating their long-lived broadcaster. The
    // suffix is read after subscription to preserve the replay-to-live boundary.
    state
        .store
        .load_task_for_principal(task_id, principal_id)
        .await?;
    let mut live = task_sender(&state, task_id).await.subscribe();
    let suffix = state
        .store
        .load_suffix(task_id, principal_id, after_sequence)
        .await?;
    outgoing
        .send(ServerMessage::Attached {
            request_id,
            task_id,
            through_sequence: suffix.through_sequence,
        })
        .await
        .map_err(|_| ControlError::invalid("surface disconnected during attachment"))?;
    for event in suffix.events {
        outgoing
            .send(ServerMessage::TaskEvent { event })
            .await
            .map_err(|_| ControlError::invalid("surface disconnected during replay"))?;
    }
    let through_sequence = suffix.through_sequence;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancelled.cancelled() => return,
                event = live.recv() => match event {
                    Ok(event) if through_sequence.is_none_or(|sequence| event.sequence > sequence) => {
                        if outgoing.send(ServerMessage::TaskEvent { event }).await.is_err() {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        send_error(
                            &outgoing,
                            None,
                            ErrorCode::ReplayRequired,
                            "surface fell behind; reconnect with its last task sequence",
                        )
                        .await;
                        return;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    });
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "these values form the complete authenticated command admission boundary"
)]
async fn submit_command(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    task_id: TaskId,
    command_id: CommandId,
    input: CommandInput,
    principal_id: PrincipalId,
    surface: SurfaceRef,
) -> Result<(), ControlError> {
    let task = state
        .store
        .load_task_for_principal(task_id, principal_id)
        .await?;
    let lifecycle = state.connection_lifecycle.lock().await;
    let node = state.nodes.lock().await.get(&task.node_id).cloned();
    let admission = state
        .store
        .admit_command(
            task_id,
            principal_id,
            surface,
            command_id,
            input,
            node.is_some(),
        )
        .await?;
    drop(lifecycle);
    let (command, event, pending) = match admission {
        CommandAdmission::NotAdmitted => {
            send_error(
                outgoing,
                Some(request_id),
                ErrorCode::NodeOffline,
                "the task's execution node is offline",
            )
            .await;
            return Ok(());
        }
        CommandAdmission::Admitted { command, event } => (command, Some(*event), true),
        CommandAdmission::Existing { command, pending } => (command, None, pending),
    };
    let _ = outgoing
        .send(ServerMessage::CommandAccepted {
            request_id,
            command_id: command.command_id,
        })
        .await;
    if let Some(event) = event {
        publish_task_event(state, event).await;
    }
    if let (true, Some(node)) = (pending, node) {
        let _ = node
            .outgoing
            .send(ServerMessage::Execute { task_id, command })
            .await;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
