use std::{collections::HashMap, collections::HashSet, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{ClientMessage, DeviceCredentials, JSON_WS_VERSION, ServerMessage, TaskId};
use renoa_protocol::{CommandEnvelope, CommandId, ExecutionEvent};
use tokio::{sync::watch, task::JoinSet};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

use crate::{
    bridge::{ExecutionTask, NodeError, NodeRuntime, finish_execution, schedule_pending},
    node_store::ExecutionRecord,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const MAX_APPLICATION_MESSAGE_BYTES: usize = 1024 * 1024;

pub(crate) enum SessionEnd {
    Disconnected,
    Shutdown,
}

struct Publication {
    record: ExecutionRecord,
    admission_in_flight: bool,
    event_in_flight: Option<u64>,
}

enum MessageResult {
    Continue,
    Disconnected,
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
    let Ok((mut socket, _)) = connect_async(endpoint).await else {
        return Ok(SessionEnd::Disconnected);
    };
    if !send_client(
        &mut socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials: credentials.clone(),
        },
    )
    .await?
    {
        return Ok(SessionEnd::Disconnected);
    }
    match receive_server(&mut socket).await? {
        Some(ServerMessage::Authenticated { version }) if version == JSON_WS_VERSION => {}
        Some(ServerMessage::Error { code, message, .. }) => {
            return Err(NodeError::Rejected { code, message });
        }
        Some(_) => {
            return Err(NodeError::Protocol(
                "coordinator did not authenticate the node".to_owned(),
            ));
        }
        None => return Ok(SessionEnd::Disconnected),
    }

    let mut publications = HashMap::new();
    refresh_publications(&runtime, &mut publications).await?;
    if !send_pending(&mut socket, &runtime, &mut publications).await? {
        return Ok(SessionEnd::Disconnected);
    }

    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(SessionEnd::Shutdown),
            changed = commits.changed() => {
                if changed.is_err() {
                    return Err(NodeError::Protocol("local commit signal closed".to_owned()));
                }
                refresh_publications(&runtime, &mut publications).await?;
                if !send_pending(&mut socket, &runtime, &mut publications).await? {
                    return Ok(SessionEnd::Disconnected);
                }
            }
            completed = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(completed) = completed {
                    finish_execution(completed, running)?;
                    schedule_pending(Arc::clone(&runtime), tasks, running).await?;
                }
            }
            message = receive_server(&mut socket) => {
                let Some(message) = message? else {
                    return Ok(SessionEnd::Disconnected);
                };
                if matches!(handle_server_message(
                    &mut socket,
                    &runtime,
                    tasks,
                    running,
                    &mut publications,
                    message,
                ).await?, MessageResult::Disconnected) {
                    return Ok(SessionEnd::Disconnected);
                }
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
) -> Result<MessageResult, NodeError> {
    match message {
        ServerMessage::Execute { task_id, command } => {
            handle_execute(runtime, tasks, running, task_id, command).await?;
            refresh_publications(runtime, publications).await?;
            if !send_pending(socket, runtime, publications).await? {
                return Ok(MessageResult::Disconnected);
            }
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
            if !send_pending(socket, runtime, publications).await? {
                return Ok(MessageResult::Disconnected);
            }
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
    Ok(MessageResult::Continue)
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
) -> Result<bool, NodeError> {
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
        let publication = publications
            .get_mut(&command_id)
            .expect("publication key must remain present");
        let record = publication.record.clone();
        let send_admission = !record.admission_acked && !publication.admission_in_flight;
        if send_admission {
            if !send_client(
                socket,
                &ClientMessage::AcknowledgeExecution {
                    task_id: record.task_id,
                    command_id,
                },
            )
            .await?
            {
                return Ok(false);
            }
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
        if !send_client(
            socket,
            &ClientMessage::PublishExecutionEvents {
                task_id: record.task_id,
                command_id,
                events,
            },
        )
        .await?
        {
            return Ok(false);
        }
        publication.event_in_flight = Some(through_execution_sequence);
        blocked_tasks.insert(record.task_id);
    }
    Ok(true)
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
            let oversized = batch.pop().expect("candidate batch contains one event");
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

async fn send_client(socket: &mut Socket, message: &ClientMessage) -> Result<bool, NodeError> {
    let json = serde_json::to_string(message)?;
    Ok(socket.send(Message::Text(json.into())).await.is_ok())
}

async fn receive_server(socket: &mut Socket) -> Result<Option<ServerMessage>, NodeError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(json))) => return Ok(Some(serde_json::from_str(&json)?)),
            Some(Ok(Message::Ping(payload))) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    return Ok(None);
                }
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_)) | Err(_)) | None => return Ok(None),
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
