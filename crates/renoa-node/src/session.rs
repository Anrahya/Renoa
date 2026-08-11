use std::{collections::HashMap, collections::HashSet, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{ClientMessage, DeviceCredentials, JSON_WS_VERSION, ServerMessage, TaskId};
use renoa_core::{CommandEnvelope, CommandId, RunStore};
use tokio::{sync::watch, task::JoinSet};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

use crate::{
    bridge::{ExecutionTask, NodeError, NodeRuntime, finish_execution, start_execution},
    node_store::ExecutionRecord,
    profile::into_execution_event,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

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
    execution_shutdown: &CancellationToken,
    commits: &mut watch::Receiver<u64>,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<CommandId>,
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
                }
            }
            message = receive_server(&mut socket) => {
                let Some(message) = message? else {
                    return Ok(SessionEnd::Disconnected);
                };
                if matches!(handle_server_message(
                    &mut socket,
                    &runtime,
                    execution_shutdown,
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
    execution_shutdown: &CancellationToken,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<CommandId>,
    publications: &mut HashMap<CommandId, Publication>,
    message: ServerMessage,
) -> Result<MessageResult, NodeError> {
    match message {
        ServerMessage::Execute { task_id, command } => {
            handle_execute(
                runtime,
                execution_shutdown,
                tasks,
                running,
                task_id,
                command,
            )
            .await?;
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
    execution_shutdown: &CancellationToken,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<CommandId>,
    task_id: TaskId,
    command: CommandEnvelope,
) -> Result<(), NodeError> {
    if let Some(existing) = runtime.state.find(command.command_id).await? {
        let transcript = runtime.run_store.load_transcript(existing.run_id).await?;
        if existing.task_id != task_id || transcript.run.command != command {
            return Err(NodeError::Protocol(format!(
                "redelivered command {} does not match its durable execution",
                command.command_id
            )));
        }
        runtime
            .state
            .require_admission_ack(command.command_id)
            .await?;
        return Ok(());
    }
    start_execution(
        Arc::clone(runtime),
        task_id,
        command,
        execution_shutdown.child_token(),
        tasks,
        running,
    );
    Ok(())
}

async fn refresh_publications(
    runtime: &NodeRuntime,
    publications: &mut HashMap<CommandId, Publication>,
) -> Result<(), NodeError> {
    for record in runtime.state.load_all().await? {
        publications
            .entry(record.command_id)
            .and_modify(|publication| publication.record = record)
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
    let command_ids = publications.keys().copied().collect::<Vec<_>>();
    for command_id in command_ids {
        let publication = publications
            .get_mut(&command_id)
            .expect("publication key must remain present");
        let record = publication.record;
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

        if publication.event_in_flight.is_some() {
            continue;
        }
        let events = runtime
            .run_store
            .load_events_after(record.run_id, record.published_through)
            .await?
            .into_iter()
            .map(into_execution_event)
            .collect::<Vec<_>>();
        let Some(through_execution_sequence) = events.last().map(|event| event.sequence) else {
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
    }
    Ok(true)
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
