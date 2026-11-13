use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_rcp_wire::{ClientMessage, MAX_APPLICATION_MESSAGE_BYTES, ServerMessage, TaskId};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

use crate::{ClientError, SurfaceEvent};

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
pub(crate) type Waiters =
    Arc<Mutex<HashMap<u64, oneshot::Sender<Result<ServerMessage, ClientError>>>>>;
pub(crate) type LiveTasks = Arc<StdMutex<HashSet<TaskId>>>;

pub(crate) async fn run(
    mut socket: Socket,
    mut outbound: mpsc::Receiver<ClientMessage>,
    events: mpsc::Sender<Result<SurfaceEvent, ClientError>>,
    waiters: Waiters,
    live_tasks: LiveTasks,
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
                    if let Err(error) = dispatch(message, &events, &waiters, &live_tasks).await {
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
    events: &mpsc::Sender<Result<SurfaceEvent, ClientError>>,
    waiters: &Waiters,
    live_tasks: &LiveTasks,
) -> Result<(), ClientError> {
    let request_id = match &message {
        ServerMessage::TaskEvent { event } => {
            return events
                .send(Ok(SurfaceEvent::Task(Box::new(event.clone()))))
                .await
                .map_err(|_| disconnected());
        }
        ServerMessage::LiveActivity {
            task_id,
            command_id,
            activity,
        } if live_tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(task_id) =>
        {
            return events
                .send(Ok(SurfaceEvent::Live {
                    task_id: *task_id,
                    command_id: *command_id,
                    activity: activity.clone(),
                }))
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
        | ServerMessage::TaskDeleted { request_id, .. }
        | ServerMessage::DeviceList { request_id, .. }
        | ServerMessage::DeviceRevoked { request_id, .. }
        | ServerMessage::Attached { request_id, .. }
        | ServerMessage::CommandAccepted { request_id, .. }
        | ServerMessage::StopAccepted { request_id, .. }
        | ServerMessage::HostAnswer { request_id, .. }
        | ServerMessage::ShareLinkIssued { request_id, .. }
        | ServerMessage::DeviceApproved { request_id, .. }
        | ServerMessage::PushTokenSet { request_id }
        | ServerMessage::PushTokenCleared { request_id }
        | ServerMessage::NoticeKeySet { request_id }
        | ServerMessage::NoticeList { request_id, .. }
        | ServerMessage::NoticeAcknowledged { request_id } => *request_id,
        ServerMessage::Enrolled { .. }
        | ServerMessage::DeviceChallenge { .. }
        | ServerMessage::Authenticated { .. }
        | ServerMessage::ExecutionEventsAccepted { .. }
        | ServerMessage::ExecutionAcknowledged { .. }
        | ServerMessage::Execute { .. }
        | ServerMessage::HostCall { .. }
        | ServerMessage::Stop { .. }
        | ServerMessage::LiveActivity { .. }
        | ServerMessage::WakeRequested { .. }
        | ServerMessage::NoticeKeys { .. }
        | ServerMessage::NoticePut { .. }
        | ServerMessage::NoticesWithdrawn { .. } => {
            return Err(ClientError::Protocol(
                "coordinator sent a node-only, session or unrequested message to a surface"
                    .to_owned(),
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

pub(crate) async fn open(endpoint: &str) -> Result<Socket, ClientError> {
    let websocket = WebSocketConfig::default()
        .max_message_size(Some(MAX_APPLICATION_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_APPLICATION_MESSAGE_BYTES));
    let (socket, _) = connect_async_with_config(endpoint, Some(websocket), false)
        .await
        .map_err(|error| ClientError::Transport(error.to_string()))?;
    Ok(socket)
}

pub(crate) async fn send(socket: &mut Socket, message: &ClientMessage) -> Result<(), ClientError> {
    let json =
        serde_json::to_string(message).map_err(|error| ClientError::Protocol(error.to_string()))?;
    socket
        .send(Message::Text(json.into()))
        .await
        .map_err(|error| ClientError::Transport(error.to_string()))
}

pub(crate) async fn receive(socket: &mut Socket) -> Result<ServerMessage, ClientError> {
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

pub(crate) fn disconnected() -> ClientError {
    ClientError::Transport("the coordinator connection ended".to_owned())
}
