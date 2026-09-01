use std::{
    collections::HashMap,
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{ClientMessage, DeviceCredentials, JSON_WS_VERSION, ServerMessage, TaskId};
use renoa_protocol::{CommandEnvelope, CommandId, ExecutionEvent};
use tokio::{sync::watch, task::JoinSet};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

use crate::{
    bridge::{ExecutionTask, NodeError, NodeRuntime, finish_execution, schedule_pending},
    node_log,
    node_store::ExecutionRecord,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const MAX_APPLICATION_MESSAGE_BYTES: usize = 1024 * 1024;

pub(crate) enum SessionEnd {
    Disconnected {
        reason: String,
        connected_for: Option<Duration>,
    },
    Shutdown,
}

struct Publication {
    record: ExecutionRecord,
    admission_in_flight: bool,
    event_in_flight: Option<u64>,
}

#[allow(
    clippy::too_many_arguments,
    reason = "these values are the complete state of one reconnectable node session"
)]
pub(crate) async fn serve_session(
    endpoint: &str,
    credentials: &DeviceCredentials,
    runtime: Arc<NodeRuntime>,
    shutdown: &CancellationToken,
    commits: &mut watch::Receiver<u64>,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<TaskId>,
) -> Result<SessionEnd, NodeError> {
    let mut authenticated_at = None;
    let result = serve_session_inner(
        endpoint,
        credentials,
        runtime,
        shutdown,
        commits,
        tasks,
        running,
        &mut authenticated_at,
    )
    .await;
    match result {
        Err(NodeError::Transport(reason)) => Ok(SessionEnd::Disconnected {
            reason,
            connected_for: authenticated_at.map(|started: Instant| started.elapsed()),
        }),
        result => result,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "these values are the complete state of one reconnectable node session"
)]
async fn serve_session_inner(
    endpoint: &str,
    credentials: &DeviceCredentials,
    runtime: Arc<NodeRuntime>,
    shutdown: &CancellationToken,
    commits: &mut watch::Receiver<u64>,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<TaskId>,
    authenticated_at: &mut Option<Instant>,
) -> Result<SessionEnd, NodeError> {
    let websocket = WebSocketConfig::default()
        .max_message_size(Some(MAX_APPLICATION_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_APPLICATION_MESSAGE_BYTES));
    let (mut socket, _) = connect_async_with_config(endpoint, Some(websocket), false)
        .await
        .map_err(|error| NodeError::Transport(error.to_string()))?;
    send_client(
        &mut socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials: credentials.clone(),
        },
    )
    .await?;
    match receive_server(&mut socket).await? {
        ServerMessage::Authenticated { version } if version == JSON_WS_VERSION => {}
        ServerMessage::Error { code, message, .. } => {
            return Err(NodeError::Rejected { code, message });
        }
        _ => {
            return Err(NodeError::Protocol(
                "coordinator did not authenticate the node".to_owned(),
            ));
        }
    }
    *authenticated_at = Some(Instant::now());
    node_log::event("info", "coordinator_connected", &serde_json::json!({}));

    let mut publications = HashMap::new();
    refresh_publications(&runtime, &mut publications).await?;
    send_pending(&mut socket, &runtime, &mut publications).await?;

    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(SessionEnd::Shutdown),
            changed = commits.changed() => {
                if changed.is_err() {
                    return Err(NodeError::Protocol("local commit signal closed".to_owned()));
                }
                refresh_publications(&runtime, &mut publications).await?;
                send_pending(&mut socket, &runtime, &mut publications).await?;
            }
            completed = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(completed) = completed {
                    finish_execution(completed, running)?;
                    schedule_pending(Arc::clone(&runtime), tasks, running).await?;
                }
            }
            message = receive_server(&mut socket) => {
                let message = message?;
                handle_server_message(
                    &mut socket,
                    &runtime,
                    tasks,
                    running,
                    &mut publications,
                    message,
                ).await?;
            }
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "these values are the complete state of one authenticated node session"
)]
async fn handle_server_message(
    socket: &mut Socket,
    runtime: &Arc<NodeRuntime>,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<TaskId>,
    publications: &mut HashMap<CommandId, Publication>,
    message: ServerMessage,
) -> Result<(), NodeError> {
    match message {
        ServerMessage::Execute { task_id, command } => {
            handle_execute(runtime, tasks, running, task_id, command).await?;
            refresh_publications(runtime, publications).await?;
            send_pending(socket, runtime, publications).await?;
        }
        ServerMessage::ExecutionAcknowledged { command_id } => {
            let publication = publications.get_mut(&command_id).ok_or_else(|| {
                NodeError::Protocol(format!(
                    "received admission acknowledgement for unknown command {command_id}"
                ))
            })?;
            if !publication.admission_in_flight {
                return Err(NodeError::Protocol(format!(
                    "received unsolicited admission acknowledgement for command {command_id}"
                )));
            }
            runtime.state.acknowledge_admission(command_id).await?;
            publication.record.admission_acked = true;
            publication.admission_in_flight = false;
            refresh_publications(runtime, publications).await?;
        }
        ServerMessage::ExecutionEventsAccepted {
            command_id,
            through_execution_sequence,
        } => {
            let publication = publications.get_mut(&command_id).ok_or_else(|| {
                NodeError::Protocol(format!(
                    "received event acknowledgement for unknown command {command_id}"
                ))
            })?;
            let expected = publication.event_in_flight.ok_or_else(|| {
                NodeError::Protocol(format!(
                    "received unsolicited event acknowledgement for command {command_id}"
                ))
            })?;
            if through_execution_sequence != expected {
                return Err(NodeError::Protocol(format!(
                    "event acknowledgement for command {command_id} ended at sequence \
                     {through_execution_sequence}, expected {expected}"
                )));
            }
            runtime
                .state
                .advance_publication(command_id, through_execution_sequence)
                .await?;
            publication.record.published_through = Some(through_execution_sequence);
            publication.event_in_flight = None;
            refresh_publications(runtime, publications).await?;
            send_pending(socket, runtime, publications).await?;
        }
        ServerMessage::Error { code, message, .. } => {
            return Err(NodeError::Rejected { code, message });
        }
        ServerMessage::Authenticated { .. }
        | ServerMessage::Enrolled { .. }
        | ServerMessage::TaskList { .. }
        | ServerMessage::Attached { .. }
        | ServerMessage::CommandAccepted { .. }
        | ServerMessage::TaskEvent { .. } => {
            return Err(NodeError::Protocol(
                "coordinator sent a surface-only message to a node".to_owned(),
            ));
        }
    }
    Ok(())
}

async fn handle_execute(
    runtime: &Arc<NodeRuntime>,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<TaskId>,
    task_id: TaskId,
    command: CommandEnvelope,
) -> Result<(), NodeError> {
    let binding = runtime.binding_for(&command.target)?;
    let command_id = command.command_id;
    runtime.state.admit(task_id, command, binding).await?;
    runtime.state.require_admission_ack(command_id).await?;
    runtime.signal_commit();
    schedule_pending(Arc::clone(runtime), tasks, running).await?;
    Ok(())
}

async fn refresh_publications(
    runtime: &NodeRuntime,
    publications: &mut HashMap<CommandId, Publication>,
) -> Result<(), NodeError> {
    let records = runtime.state.load_pending_publications().await?;
    let pending = records
        .iter()
        .map(|record| record.command.command_id)
        .collect::<HashSet<_>>();
    publications.retain(|command_id, _| pending.contains(command_id));
    for record in records {
        publications
            .entry(record.command.command_id)
            .and_modify(|publication| publication.record = record.clone())
            .or_insert(Publication {
                record,
                admission_in_flight: false,
                event_in_flight: None,
            });
    }
    Ok(())
}

async fn send_pending(
    socket: &mut Socket,
    runtime: &NodeRuntime,
    publications: &mut HashMap<CommandId, Publication>,
) -> Result<(), NodeError> {
    let mut ordered = publications
        .iter()
        .map(|(command_id, publication)| {
            (
                publication.record.task_id.to_string(),
                publication.record.admission_sequence,
                *command_id,
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_unstable_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut blocked_tasks = HashSet::new();
    for (_, _, command_id) in ordered {
        let publication = publications.get_mut(&command_id).ok_or_else(|| {
            NodeError::Protocol(format!(
                "pending publication {command_id} disappeared during scheduling"
            ))
        })?;
        let record = publication.record.clone();
        let send_admission = !record.admission_acked && !publication.admission_in_flight;
        if send_admission {
            send_client(
                socket,
                &ClientMessage::AcknowledgeExecution {
                    task_id: record.task_id,
                    command_id,
                },
            )
            .await?;
            publication.admission_in_flight = true;
        }

        if blocked_tasks.contains(&record.task_id) {
            continue;
        }
        if publication.event_in_flight.is_some() {
            blocked_tasks.insert(record.task_id);
            continue;
        }
        let events = runtime
            .state
            .load_events_after(command_id, record.published_through)
            .await?;
        let events = publication_batch(record.task_id, command_id, events)?;
        let Some(through_execution_sequence) = events.last().map(|event| event.sequence) else {
            if !record.terminal {
                blocked_tasks.insert(record.task_id);
            }
            continue;
        };
        send_client(
            socket,
            &ClientMessage::PublishExecutionEvents {
                task_id: record.task_id,
                command_id,
                events,
            },
        )
        .await?;
        publication.event_in_flight = Some(through_execution_sequence);
        blocked_tasks.insert(record.task_id);
    }
    Ok(())
}

fn publication_batch(
    task_id: TaskId,
    command_id: CommandId,
    events: Vec<ExecutionEvent>,
) -> Result<Vec<ExecutionEvent>, NodeError> {
    let mut batch = Vec::new();
    for event in events {
        batch.push(event);
        let message = ClientMessage::PublishExecutionEvents {
            task_id,
            command_id,
            events: batch.clone(),
        };
        if serde_json::to_vec(&message)?.len() > MAX_APPLICATION_MESSAGE_BYTES {
            let oversized = batch.pop().ok_or_else(|| {
                NodeError::Protocol("publication batch lost its candidate event".to_owned())
            })?;
            if batch.is_empty() {
                return Err(NodeError::Protocol(format!(
                    "execution event {} exceeds the RCP WebSocket message limit",
                    oversized.event_id
                )));
            }
            break;
        }
    }
    Ok(batch)
}

async fn send_client(socket: &mut Socket, message: &ClientMessage) -> Result<(), NodeError> {
    let json = serde_json::to_string(message)?;
    socket
        .send(Message::Text(json.into()))
        .await
        .map_err(|error| NodeError::Transport(error.to_string()))
}

async fn receive_server(socket: &mut Socket) -> Result<ServerMessage, NodeError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(json))) => return Ok(serde_json::from_str(&json)?),
            Some(Ok(Message::Ping(payload))) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    return Err(NodeError::Transport(
                        "connection closed while answering a ping".to_owned(),
                    ));
                }
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(frame))) => {
                return Err(NodeError::Transport(match frame {
                    Some(frame) => format!("coordinator closed: {}", frame.reason),
                    None => "coordinator closed the WebSocket".to_owned(),
                }));
            }
            Some(Err(error)) => return Err(NodeError::Transport(error.to_string())),
            None => {
                return Err(NodeError::Transport(
                    "coordinator connection ended".to_owned(),
                ));
            }
            Some(Ok(Message::Binary(_) | Message::Frame(_))) => {
                return Err(NodeError::Protocol(
                    "coordinator sent a non-JSON WebSocket message".to_owned(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use renoa_protocol::{ExecutionEventId, ExecutionEventKind, ExecutionId};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn publication_batches_stop_before_the_websocket_limit() {
        let task_id = TaskId::new();
        let command_id = CommandId::new();
        let execution_id = ExecutionId::from_uuid(Uuid::new_v4());
        let events = vec![
            event(execution_id, 0, ExecutionEventKind::ExecutionStarted),
            event(
                execution_id,
                1,
                ExecutionEventKind::AssistantMessage {
                    text: "a".repeat(600_000),
                },
            ),
            event(
                execution_id,
                2,
                ExecutionEventKind::AssistantMessage {
                    text: "b".repeat(600_000),
                },
            ),
        ];

        let batch = publication_batch(task_id, command_id, events).expect("build batch");

        assert_eq!(batch.len(), 2);
        let encoded = serde_json::to_vec(&ClientMessage::PublishExecutionEvents {
            task_id,
            command_id,
            events: batch,
        })
        .expect("encode batch");
        assert!(encoded.len() <= MAX_APPLICATION_MESSAGE_BYTES);
    }

    #[test]
    fn one_oversized_execution_event_fails_explicitly() {
        let event = event(
            ExecutionId::from_uuid(Uuid::new_v4()),
            1,
            ExecutionEventKind::AssistantMessage {
                text: "x".repeat(MAX_APPLICATION_MESSAGE_BYTES),
            },
        );

        let error = publication_batch(TaskId::new(), CommandId::new(), vec![event])
            .expect_err("oversized event must fail");

        assert!(matches!(error, NodeError::Protocol(_)));
    }

    fn event(execution_id: ExecutionId, sequence: u64, kind: ExecutionEventKind) -> ExecutionEvent {
        ExecutionEvent {
            event_id: ExecutionEventId::new(),
            execution_id,
            sequence,
            recorded_at_ms: 1,
            kind,
        }
    }
}
