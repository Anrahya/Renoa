use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use axum::{Router, routing::get};
use renoa_protocol::{PrincipalId, TargetRef};
use thiserror::Error;
use tokio::{
    net::TcpListener,
    sync::{Mutex, Semaphore, broadcast, mpsc},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    DeviceId, ErrorCode, NodeId, PasskeyBootstrapToken, PeerIdentity, ServerMessage, TaskEvent,
    TaskId, android_apps::AndroidApps, browser_identity::BrowserIdentity, browser_identity_http,
    connection::upgrade_connection, oauth_relay_http, store::ControlStore,
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
    NodeOffline,
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

    pub(crate) fn node_offline() -> Self {
        Self::new(
            ControlErrorKind::NodeOffline,
            "the task's execution node is offline",
        )
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
            ControlErrorKind::NodeOffline => ErrorCode::NodeOffline,
            ControlErrorKind::NotFound => ErrorCode::NotFound,
        }
    }
}

#[derive(Clone)]
pub struct Coordinator {
    state: Arc<CoordinatorState>,
    control_room: Option<PathBuf>,
}

pub(crate) struct CoordinatorState {
    pub(crate) browser_identity: Option<BrowserIdentity>,
    pub(crate) android_apps: AndroidApps,
    pub(crate) browser_sessions: crate::BrowserSessions,
    pub(crate) oauth_callback_uri: Option<String>,
    pub(crate) connection_slots: Arc<Semaphore>,
    pub(crate) connection_lifecycle: Mutex<()>,
    pub(crate) store: ControlStore,
    pub(crate) host_calls: crate::host_calls::HostCalls,
    pub(crate) nodes: Mutex<HashMap<NodeId, NodeConnection>>,
    pub(crate) sessions: Mutex<HashMap<DeviceId, HashMap<Uuid, CancellationToken>>>,
    pub(crate) task_senders: Mutex<HashMap<TaskId, broadcast::Sender<TaskEvent>>>,
    pub(crate) live_channels: Mutex<HashMap<TaskId, crate::live_activity::LiveChannel>>,
}

#[derive(Clone)]
pub(crate) struct NodeConnection {
    pub(crate) connection_id: Uuid,
    pub(crate) device_id: DeviceId,
    pub(crate) outgoing: mpsc::Sender<ServerMessage>,
    /// The agents this connection advertised; empty until it advertises.
    pub(crate) targets: Vec<TargetRef>,
    /// Whether this connection said it answers Host control calls.
    pub(crate) host_control: bool,
}

impl Coordinator {
    /// Opens the coordinator's durable task journal.
    ///
    /// # Errors
    ///
    /// Returns an error when the `SQLite` journal cannot be opened or initialized.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ControlError> {
        Self::open_inner(path, None, None, AndroidApps::default())
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
        Self::open_with_android_apps(path, rp_id, rp_origin, AndroidApps::default())
    }

    /// Opens the coordinator with passkey authentication that also accepts the
    /// configured Android apps' origins and serves their asset links.
    ///
    /// # Errors
    ///
    /// Returns an error when the database or passkey relying-party configuration is invalid.
    pub fn open_with_android_apps(
        path: impl AsRef<Path>,
        rp_id: &str,
        rp_origin: &str,
        android_apps: AndroidApps,
    ) -> Result<Self, ControlError> {
        let browser_identity = BrowserIdentity::new(rp_id, rp_origin, &android_apps.origins()?)?;
        let oauth_callback_uri = format!(
            "{}{}",
            rp_origin.trim_end_matches('/'),
            renoa_oauth_relay_protocol::OAUTH_CALLBACK_PATH
        );
        Self::open_inner(
            path,
            Some(browser_identity),
            Some(oauth_callback_uri),
            android_apps,
        )
    }

    fn open_inner(
        path: impl AsRef<Path>,
        browser_identity: Option<BrowserIdentity>,
        oauth_callback_uri: Option<String>,
        android_apps: AndroidApps,
    ) -> Result<Self, ControlError> {
        let store = ControlStore::open(path)?;
        let browser_sessions = crate::BrowserSessions::open(store.path.as_ref())?;
        Ok(Self {
            control_room: None,
            state: Arc::new(CoordinatorState {
                browser_identity,
                android_apps,
                browser_sessions,
                oauth_callback_uri,
                connection_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS)),
                connection_lifecycle: Mutex::new(()),
                store,
                host_calls: crate::host_calls::HostCalls::default(),
                nodes: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                task_senders: Mutex::new(HashMap::new()),
                live_channels: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Serves the built Control Room from `directory` on the coordinator's own
    /// origin, beside the API.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory has no `index.html`.
    pub fn with_control_room(mut self, directory: impl AsRef<Path>) -> Result<Self, ControlError> {
        self.control_room = Some(crate::control_room::check(directory.as_ref())?);
        Ok(self)
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

    /// Records `owner` as the node's owner and creates its single-use
    /// enrollment. Only the owner may open new tasks on the node.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the node already belongs to another principal,
    /// or an error when the enrollment cannot be persisted.
    pub async fn create_node_enrollment(
        &self,
        node_id: NodeId,
        owner: PrincipalId,
        expires_at: SystemTime,
    ) -> Result<crate::EnrollmentToken, ControlError> {
        self.state
            .store
            .create_node_enrollment(node_id, owner, expires_at)
            .await
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
        crate::device_admin::end_device_sessions(&self.state, device_id).await;
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
            app = app
                .merge(browser_identity_http::routes())
                .merge(crate::browser_sessions_http::routes())
                .merge(oauth_relay_http::routes())
                .merge(crate::credential_relay_http::routes())
                .merge(crate::host_control_http::routes());
        }
        if let Some(directory) = self.control_room {
            app = app.fallback_service(crate::control_room::service(directory));
        }
        if !self.state.android_apps.is_empty() {
            app = app.merge(crate::android_apps::routes());
        }
        let app = app.with_state(Arc::clone(&self.state));
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .map_err(|error| ControlError::store(format!("coordinator server failed: {error}")))
    }
}

#[cfg(test)]
mod tests;
