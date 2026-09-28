//! A Rust surface's authenticated link to the RCP coordinator.
//!
//! The client owns only the transport: it correlates requests with their
//! responses and delivers task events in socket order. Durable surface state,
//! such as task cursors and unsent commands, belongs to the surface, which
//! reconnects and resumes from it.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_control::{
    ClientMessage, DeviceCredentials, EnrollmentToken, ErrorCode, JSON_WS_VERSION, NodeId,
    ServerMessage, TargetSummary, TaskEvent, TaskId,
};
use renoa_protocol::{CommandId, CommandInput, TargetRef};
use thiserror::Error;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type Waiters = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<ServerMessage, ClientError>>>>>;

const MAX_APPLICATION_MESSAGE_BYTES: usize = 1024 * 1024;
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
}

impl ClientError {
    /// The coordinator's error code, when it refused the operation.
    #[must_use]
    pub const fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Rejected { code, .. } => Some(*code),
            Self::Transport(_) | Self::Protocol(_) => None,
        }
    }
}

/// Exchanges a single-use enrollment token for this surface's device credential.
///
/// # Errors
///
/// Returns a transport failure or the coordinator's refusal.
pub async fn enroll(
    endpoint: &str,
    token: EnrollmentToken,
) -> Result<DeviceCredentials, ClientError> {
    let mut socket = open(endpoint).await?;
    send(
        &mut socket,
        &ClientMessage::Enroll {
            version: JSON_WS_VERSION,
            token,
        },
    )
    .await?;
    match receive(&mut socket).await? {
        ServerMessage::Enrolled {
            version,
            credentials,
        } if version == JSON_WS_VERSION => Ok(credentials),
        ServerMessage::Error { code, message, .. } => Err(ClientError::Rejected { code, message }),
        _ => Err(ClientError::Protocol(
            "coordinator did not return a device credential".to_owned(),
        )),
    }
}

/// One authenticated surface connection. Dropping every handle closes it.
#[derive(Clone)]
pub struct Connection {
    outgoing: mpsc::Sender<ClientMessage>,
    waiters: Waiters,
    next_request: Arc<AtomicU64>,
    closed: CancellationToken,
}

/// Task events in coordinator order. The stream ends, after reporting the
/// reason, when the connection ends.
pub struct Events(mpsc::Receiver<Result<TaskEvent, ClientError>>);

impl Events {
    /// Returns the next task event, or `None` after the connection's final error.
    pub async fn next(&mut self) -> Option<Result<TaskEvent, ClientError>> {
        self.0.recv().await
    }
}

/// Authenticates with a device credential and starts the connection.
///
/// # Errors
///
/// Returns a transport failure or the coordinator's refusal.
pub async fn connect(
    endpoint: &str,
    credentials: DeviceCredentials,
) -> Result<(Connection, Events), ClientError> {
    let mut socket = open(endpoint).await?;
    send(
        &mut socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials,
        },
    )
    .await?;
    match receive(&mut socket).await? {
        ServerMessage::Authenticated { version } if version == JSON_WS_VERSION => {}
        ServerMessage::Error { code, message, .. } => {
            return Err(ClientError::Rejected { code, message });
        }
        _ => {
            return Err(ClientError::Protocol(
                "coordinator did not authenticate the surface".to_owned(),
            ));
        }
    }
    let (outgoing, outbound) = mpsc::channel(OUTBOUND_CAPACITY);
    let (events, inbound) = mpsc::channel(EVENT_CAPACITY);
    let connection = Connection {
        outgoing,
        waiters: Arc::default(),
        next_request: Arc::new(AtomicU64::new(1)),
        closed: CancellationToken::new(),
    };
    tokio::spawn(run(
        socket,
        outbound,
        events,
        Arc::clone(&connection.waiters),
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

    /// Attaches to a task after `after_sequence`. The replayed records and then
    /// live records arrive on [`Events`]. Returns the replay high-water mark.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal.
    pub async fn attach(
        &self,
        task_id: TaskId,
        after_sequence: Option<u64>,
    ) -> Result<Option<u64>, ClientError> {
        match self
            .request(|request_id| ClientMessage::Attach {
                request_id,
                task_id,
                after_sequence,
            })
            .await?
        {
            ServerMessage::Attached {
                task_id: attached,
                through_sequence,
                ..
            } if attached == task_id => Ok(through_sequence),
            other => Err(unexpected("attached", &other)),
        }
    }

    /// Submits one text command under a stable identity. An exact retry of an
    /// admitted command is accepted again without a second record.
    ///
    /// # Errors
    ///
    /// Returns a transport failure or the coordinator's refusal, such as
    /// `NodeOffline` for a new command whose node is unavailable.
    pub async fn submit(
        &self,
        task_id: TaskId,
        command_id: CommandId,
        text: String,
    ) -> Result<(), ClientError> {
        match self
            .request(|request_id| ClientMessage::Submit {
                request_id,
                task_id,
                command_id,
                input: CommandInput::Text { text },
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

async fn run(
    mut socket: Socket,
    mut outbound: mpsc::Receiver<ClientMessage>,
    events: mpsc::Sender<Result<TaskEvent, ClientError>>,
    waiters: Waiters,
    closed: CancellationToken,
) {
    let reason = loop {
        tokio::select! {
            message = outbound.recv() => {
                let Some(message) = message else { break disconnected() };
                if let Err(error) = send(&mut socket, &message).await {
                    break error;
                }
            }
            message = receive(&mut socket) => match message {
                Ok(message) => {
                    if let Err(error) = dispatch(message, &events, &waiters).await {
                        break error;
                    }
                }
                Err(error) => break error,
            }
        }
    };
    closed.cancel();
    for (_, waiter) in waiters.lock().await.drain() {
        let _ = waiter.send(Err(reason.clone()));
    }
    let _ = events.send(Err(reason)).await;
    let _ = socket.close(None).await;
}

async fn dispatch(
    message: ServerMessage,
    events: &mpsc::Sender<Result<TaskEvent, ClientError>>,
    waiters: &Waiters,
) -> Result<(), ClientError> {
    let request_id = match &message {
        ServerMessage::TaskEvent { event } => {
            return events
                .send(Ok(event.clone()))
                .await
                .map_err(|_| disconnected());
        }
        ServerMessage::Error {
            request_id: None,
            code,
            message,
        } => {
            return Err(ClientError::Rejected {
                code: *code,
                message: message.clone(),
            });
        }
        ServerMessage::Error {
            request_id: Some(request_id),
            ..
        }
        | ServerMessage::TaskList { request_id, .. }
        | ServerMessage::TargetList { request_id, .. }
        | ServerMessage::TaskOpened { request_id, .. }
        | ServerMessage::Attached { request_id, .. }
        | ServerMessage::CommandAccepted { request_id, .. } => *request_id,
        ServerMessage::Enrolled { .. }
        | ServerMessage::Authenticated { .. }
        | ServerMessage::ExecutionEventsAccepted { .. }
        | ServerMessage::ExecutionAcknowledged { .. }
        | ServerMessage::Execute { .. } => {
            return Err(ClientError::Protocol(
                "coordinator sent a node-only or session message to a surface".to_owned(),
            ));
        }
    };
    let Some(waiter) = waiters.lock().await.remove(&request_id) else {
        return Err(ClientError::Protocol(format!(
            "coordinator answered unknown request {request_id}"
        )));
    };
    let result = match message {
        ServerMessage::Error { code, message, .. } => Err(ClientError::Rejected { code, message }),
        message => Ok(message),
    };
    let _ = waiter.send(result);
    Ok(())
}

async fn open(endpoint: &str) -> Result<Socket, ClientError> {
    let websocket = WebSocketConfig::default()
        .max_message_size(Some(MAX_APPLICATION_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_APPLICATION_MESSAGE_BYTES));
    let (socket, _) = connect_async_with_config(endpoint, Some(websocket), false)
        .await
        .map_err(|error| ClientError::Transport(error.to_string()))?;
    Ok(socket)
}

async fn send(socket: &mut Socket, message: &ClientMessage) -> Result<(), ClientError> {
    let json =
        serde_json::to_string(message).map_err(|error| ClientError::Protocol(error.to_string()))?;
    socket
        .send(Message::Text(json.into()))
        .await
        .map_err(|error| ClientError::Transport(error.to_string()))
}

async fn receive(socket: &mut Socket) -> Result<ServerMessage, ClientError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(json))) => {
                return serde_json::from_str(&json)
                    .map_err(|error| ClientError::Protocol(error.to_string()));
            }
            Some(Ok(Message::Ping(payload))) => {
                socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| ClientError::Transport(error.to_string()))?;
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | None => return Err(disconnected()),
            Some(Err(error)) => return Err(ClientError::Transport(error.to_string())),
            Some(Ok(Message::Binary(_) | Message::Frame(_))) => {
                return Err(ClientError::Protocol(
                    "coordinator sent a non-text frame".to_owned(),
                ));
            }
        }
    }
}

fn disconnected() -> ClientError {
    ClientError::Transport("the coordinator connection ended".to_owned())
}

fn unexpected(expected: &str, received: &ServerMessage) -> ClientError {
    ClientError::Protocol(format!("expected {expected}, received {received:?}"))
}
