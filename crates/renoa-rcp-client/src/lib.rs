//! A Rust surface's authenticated link to the RCP coordinator.
//!
//! The client owns only the transport: it correlates requests with their
//! responses and delivers task events in socket order. Durable surface state,
//! such as task cursors and unsent commands, belongs to the surface, which
//! reconnects and resumes from it.

mod handshake;
mod sign_in;
mod transport;

pub use handshake::enroll;

pub use sign_in::{Collected, DeviceKey, SignInRequest, ask_to_sign_in, collect_sign_in};

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex as StdMutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use renoa_protocol::{CommandId, CommandInput, LiveActivity, TargetRef};
use renoa_rcp_wire::{
    ClientMessage, DeviceCredentials, ErrorCode, HostMethod, NodeId, ServerMessage, TargetSummary,
    TaskEvent, TaskId,
};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use transport::{LiveTasks, Socket, Waiters, disconnected, open, receive, run, send};

const OUTBOUND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 256;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ClientError {
    /// The connection failed or ended; reconnecting may succeed.
    #[error("RCP transport failed: {0}")]
    Transport(String),
    /// The coordinator refused the operation.
    #[error("coordinator refused ({code:?}): {message}")]
    Rejected { code: ErrorCode, message: String },
    /// The coordinator sent something this client cannot interpret.
    #[error("RCP protocol error: {0}")]
    Protocol(String),
    /// The coordinator is holding as many sign-in requests as it takes; asking
    /// again once one expires may succeed.
    #[error("coordinator is busy: {0}")]
    Busy(String),
    /// The device key could not sign.
    #[error("device key failed: {0}")]
    Key(String),
}

impl ClientError {
    /// The coordinator's error code, when it refused the operation.
    #[must_use]
    pub const fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Rejected { code, .. } => Some(*code),
            Self::Transport(_) | Self::Protocol(_) | Self::Busy(_) | Self::Key(_) => None,
        }
    }
}

/// One authenticated surface connection. Dropping every handle closes it.
#[derive(Clone)]
pub struct Connection {
    outgoing: mpsc::Sender<ClientMessage>,
    waiters: Waiters,
    live_tasks: LiveTasks,
    next_request: Arc<AtomicU64>,
    closed: CancellationToken,
}

/// The Host's answer to [`Connection::call_host`]: its HTTP status, a
/// success or the Host's own refusal, and its JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAnswer {
    pub status: u16,
    pub body: String,
}

/// One coordinator-pushed task record or ephemeral live activity.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceEvent {
    Task(Box<TaskEvent>),
    Live {
        task_id: TaskId,
        command_id: CommandId,
        activity: LiveActivity,
    },
}

/// Task events in coordinator order. The stream ends, after reporting the
/// reason, when the connection ends.
pub struct Events(mpsc::Receiver<Result<SurfaceEvent, ClientError>>);

impl Events {
    /// Returns the next task event, skipping ephemeral live activity frames, or
    /// `None` after the connection's final error.
    pub async fn next(&mut self) -> Option<Result<TaskEvent, ClientError>> {
        while let Some(item) = self.0.recv().await {
            match item {
                Ok(SurfaceEvent::Task(event)) => return Some(Ok(*event)),
                Ok(SurfaceEvent::Live { .. }) => {}
                Err(error) => return Some(Err(error)),
            }
        }
        None
    }

    /// Returns the next task record or live activity frame in socket order, or
    /// `None` after the connection's final error.
    pub async fn next_event(&mut self) -> Option<Result<SurfaceEvent, ClientError>> {
        self.0.recv().await
    }
}

/// Authenticates with a device credential that has no key and starts the
/// connection.
///
/// # Errors
///
/// Returns a transport failure or the coordinator's refusal, and a protocol
/// error for a device that registered a key, which connects with
/// [`connect_with_key`].
pub async fn connect(
    endpoint: &str,
    credentials: DeviceCredentials,
) -> Result<(Connection, Events), ClientError> {
    start(endpoint, credentials, None).await
}

/// Authenticates with a key-bound device's credential, answers the
/// coordinator's challenge with `key`, and starts the connection.
///
/// # Errors
///
/// Returns a transport failure, the key's failure, or the coordinator's
/// refusal.
pub async fn connect_with_key(
    endpoint: &str,
    credentials: DeviceCredentials,
    key: &dyn DeviceKey,
) -> Result<(Connection, Events), ClientError> {
    start(endpoint, credentials, Some(key)).await
}

async fn start(
    endpoint: &str,
    credentials: DeviceCredentials,
    key: Option<&dyn DeviceKey>,
) -> Result<(Connection, Events), ClientError> {
    let mut socket = open(endpoint).await?;
    handshake::authenticate(&mut socket, credentials, key).await?;
    let (outgoing, outbound) = mpsc::channel(OUTBOUND_CAPACITY);
    let (events, inbound) = mpsc::channel(EVENT_CAPACITY);
    let live_tasks: LiveTasks = Arc::new(StdMutex::new(HashSet::new()));
    let connection = Connection {
        outgoing,
        waiters: Arc::default(),
        live_tasks: Arc::clone(&live_tasks),
        next_request: Arc::new(AtomicU64::new(1)),
        closed: CancellationToken::new(),
    };
    tokio::spawn(run(
        socket,
        outbound,
        events,
        Arc::clone(&connection.waiters),
        live_tasks,
        connection.closed.clone(),
    ));
    Ok((connection, Events(inbound)))
}

impl Connection {
    /// Lists the targets advertised by online nodes this principal owns.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal.
    pub async fn list_targets(&self) -> Result<Vec<TargetSummary>, ClientError> {
        match self
            .request(|request_id| ClientMessage::ListTargets { request_id })
            .await?
        {
            ServerMessage::TargetList { targets, .. } => Ok(targets),
            other => Err(unexpected("target_list", &other)),
        }
    }

    /// Opens a task, or converges on an exact earlier opening of `task_id`.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal, such as
    /// `NodeOffline` or `NotFound`.
    pub async fn open_task(
        &self,
        task_id: TaskId,
        node_id: NodeId,
        target: TargetRef,
    ) -> Result<(), ClientError> {
        match self
            .request(|request_id| ClientMessage::OpenTask {
                request_id,
                task_id,
                node_id,
                target,
            })
            .await?
        {
            ServerMessage::TaskOpened {
                task_id: opened, ..
            } if opened == task_id => Ok(()),
            other => Err(unexpected("task_opened", &other)),
        }
    }

    /// Deletes a task this principal owns with everything the coordinator
    /// holds for it. Deleting a task that no longer exists succeeds.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal: `Conflict`
    /// while a command of the task has not terminated, `NotFound` for another
    /// principal's task.
    pub async fn delete_task(&self, task_id: TaskId) -> Result<(), ClientError> {
        match self
            .request(|request_id| ClientMessage::DeleteTask {
                request_id,
                task_id,
            })
            .await?
        {
            ServerMessage::TaskDeleted {
                task_id: deleted, ..
            } if deleted == task_id => Ok(()),
            other => Err(unexpected("task_deleted", &other)),
        }
    }

    /// Attaches to a task after `after_sequence`. The replayed records and then
    /// live records arrive on [`Events`]. Returns the replay high-water mark.
    /// This attachment does not ask for the task's ephemeral live activity.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal.
    pub async fn attach(
        &self,
        task_id: TaskId,
        after_sequence: Option<u64>,
    ) -> Result<Option<u64>, ClientError> {
        self.attach_with_live(task_id, after_sequence, false).await
    }

    /// Attaches to a task after `after_sequence` and subscribes to ephemeral
    /// live activity frames on [`Events::next_event`]. Returns the replay
    /// high-water mark.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal.
    pub async fn attach_live(
        &self,
        task_id: TaskId,
        after_sequence: Option<u64>,
    ) -> Result<Option<u64>, ClientError> {
        self.attach_with_live(task_id, after_sequence, true).await
    }

    async fn attach_with_live(
        &self,
        task_id: TaskId,
        after_sequence: Option<u64>,
        live: bool,
    ) -> Result<Option<u64>, ClientError> {
        let was_live = if live {
            !self
                .live_tasks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(task_id)
        } else {
            false
        };
        let attached = match self
            .request(|request_id| ClientMessage::Attach {
                request_id,
                task_id,
                after_sequence,
                live,
            })
            .await
        {
            Ok(ServerMessage::Attached {
                task_id: attached,
                through_sequence,
                ..
            }) if attached == task_id => Ok(through_sequence),
            Ok(other) => Err(unexpected("attached", &other)),
            Err(error) => Err(error),
        };
        let mut guard = self
            .live_tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match (&attached, live) {
            (Ok(_), false) => {
                guard.remove(&task_id);
            }
            (Err(_), true) if !was_live => {
                guard.remove(&task_id);
            }
            _ => {}
        }
        attached
    }

    /// Submits one command under a stable identity. An exact retry of an
    /// admitted command, with identical input, is accepted again without a
    /// second record.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal, such as
    /// `NodeOffline` for a new command whose node is unavailable.
    pub async fn submit(
        &self,
        task_id: TaskId,
        command_id: CommandId,
        input: CommandInput,
    ) -> Result<(), ClientError> {
        match self
            .request(|request_id| ClientMessage::Submit {
                request_id,
                task_id,
                command_id,
                input,
            })
            .await?
        {
            ServerMessage::CommandAccepted {
                command_id: accepted,
                ..
            } if accepted == command_id => Ok(()),
            other => Err(unexpected("command_accepted", &other)),
        }
    }

    /// Calls the owner's Host at `path` under `/v1/host`, as a browser does
    /// there. Only a connection that proved its device key may.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal: `NodeOffline`
    /// when the Host could not be reached or did not answer, in which case a
    /// change may or may not have taken effect.
    pub async fn call_host(
        &self,
        method: HostMethod,
        path: &str,
        body: Option<String>,
    ) -> Result<HostAnswer, ClientError> {
        match self
            .request(|request_id| ClientMessage::CallHost {
                request_id,
                method,
                path: path.to_owned(),
                body,
            })
            .await?
        {
            ServerMessage::HostAnswer { status, body, .. } => Ok(HostAnswer { status, body }),
            other => Err(unexpected("host_answer", &other)),
        }
    }

    /// Resolves when the connection has ended.
    pub async fn closed(&self) {
        self.closed.cancelled().await;
    }

    async fn request(
        &self,
        message: impl FnOnce(u64) -> ClientMessage,
    ) -> Result<ServerMessage, ClientError> {
        let request_id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (response, waiting) = oneshot::channel();
        self.waiters.lock().await.insert(request_id, response);
        if self.outgoing.send(message(request_id)).await.is_err() {
            self.waiters.lock().await.remove(&request_id);
            return Err(disconnected());
        }
        waiting.await.unwrap_or_else(|_| Err(disconnected()))
    }
}

fn unexpected(expected: &str, received: &ServerMessage) -> ClientError {
    ClientError::Protocol(format!("expected {expected}, received {received:?}"))
}
