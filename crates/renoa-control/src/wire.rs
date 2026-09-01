use axum::extract::ws::Message;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::{
    ClientMessage, ControlError, ErrorCode, PeerIdentity, ServerMessage, TaskEvent, TaskId,
    coordinator::CoordinatorState,
};

pub(crate) const TASK_BROADCAST_CAPACITY: usize = 256;

pub(crate) enum InboundMessage {
    Application(ClientMessage),
    Control,
    Closed,
    Invalid,
}

pub(crate) async fn publish_task_event(state: &CoordinatorState, event: TaskEvent) {
    let _ = task_sender(state, event.task_id).await.send(event);
}

pub(crate) async fn task_sender(
    state: &CoordinatorState,
    task_id: TaskId,
) -> broadcast::Sender<TaskEvent> {
    state
        .task_senders
        .lock()
        .await
        .entry(task_id)
        .or_insert_with(|| broadcast::channel(TASK_BROADCAST_CAPACITY).0)
        .clone()
}

pub(crate) async fn cleanup_connection(
    state: &CoordinatorState,
    peer: &PeerIdentity,
    connection_id: Uuid,
) {
    let PeerIdentity::Node { node_id } = peer else {
        return;
    };
    let mut nodes = state.nodes.lock().await;
    if nodes
        .get(node_id)
        .is_some_and(|node| node.connection_id == connection_id)
    {
        nodes.remove(node_id);
    }
}

pub(crate) fn classify_message(message: Message) -> InboundMessage {
    match message {
        Message::Text(json) => serde_json::from_str::<ClientMessage>(&json)
            .ok()
            .filter(ClientMessage::has_interoperable_numbers)
            .map_or(InboundMessage::Invalid, InboundMessage::Application),
        Message::Ping(_) | Message::Pong(_) => InboundMessage::Control,
        Message::Close(_) => InboundMessage::Closed,
        Message::Binary(_) => InboundMessage::Invalid,
    }
}

pub(crate) async fn send_control_error(
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: Option<u64>,
    error: &ControlError,
) {
    let code = error.protocol_code();
    let message = if code == ErrorCode::Internal {
        "internal coordinator error".to_owned()
    } else {
        error.to_string()
    };
    send_error(outgoing, request_id, code, message).await;
}

pub(crate) async fn send_error(
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: Option<u64>,
    code: ErrorCode,
    message: impl Into<String>,
) {
    let _ = outgoing
        .send(ServerMessage::Error {
            request_id,
            code,
            message: message.into(),
        })
        .await;
}
