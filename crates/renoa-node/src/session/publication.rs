//! The execution events a node still owes the coordinator: which pending
//! records are in flight, and the batches that fit one WebSocket message.

use std::collections::{HashMap, HashSet};

use renoa_control::{ClientMessage, TaskId};
use renoa_protocol::{CommandId, ExecutionEvent};

use super::{MAX_APPLICATION_MESSAGE_BYTES, Socket, send_client};
use crate::{
    bridge::{NodeError, NodeRuntime},
    node_store::ExecutionRecord,
};

pub(super) struct Publication {
    pub(super) record: ExecutionRecord,
    pub(super) admission_in_flight: bool,
    pub(super) event_in_flight: Option<u64>,
}

pub(super) async fn refresh_publications(
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

pub(super) async fn send_pending(
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

pub(super) fn publication_batch(
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
