//! The execution events a node still owes the coordinator: which pending
//! records are in flight, and the batches that fit one WebSocket message. The
//! live frames that precede a record travel ahead of it.

use std::collections::{HashMap, HashSet};

use renoa_control::{ClientMessage, TaskId};
use renoa_protocol::{CommandId, ExecutionEvent};
use tokio::sync::broadcast::{self, error::TryRecvError};

use super::{MAX_APPLICATION_MESSAGE_BYTES, Socket, send_client};
use crate::{
    bridge::{LIVE_FRAME_CAPACITY, NodeError, NodeRuntime},
    live::LiveFrame,
    node_store::{ExecutionRecord, NodeStore},
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

/// Sends one live frame to the coordinator.
pub(super) async fn send_live(socket: &mut Socket, frame: LiveFrame) -> Result<(), NodeError> {
    send_client(
        socket,
        &ClientMessage::PublishLiveActivity {
            task_id: frame.task_id,
            command_id: frame.command_id,
            activity: frame.activity,
        },
    )
    .await
}

/// Sends the live frames already queued. A frame is queued before the record
/// that supersedes it is committed, so a record that was read from the ledger
/// is never sent ahead of a frame that preceded it.
async fn send_waiting_live(
    socket: &mut Socket,
    live: &mut broadcast::Receiver<LiveFrame>,
) -> Result<(), NodeError> {
    // Bounded by what the channel holds (and the lag report that may precede
    // it): frames published while this runs follow the record.
    for _ in 0..=LIVE_FRAME_CAPACITY {
        match live.try_recv() {
            Ok(frame) => send_live(socket, frame).await?,
            // Dropped frames are the surface's gap to notice.
            Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Closed) => {
                return Err(NodeError::Protocol("local live channel closed".to_owned()));
            }
        }
    }
    Ok(())
}

pub(super) async fn send_pending(
    socket: &mut Socket,
    state: &NodeStore,
    live: &mut broadcast::Receiver<LiveFrame>,
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
        let events = state
            .load_events_after(command_id, record.published_through)
            .await?;
        let events = publication_batch(record.task_id, command_id, events)?;
        let Some(through_execution_sequence) = events.last().map(|event| event.sequence) else {
            if !record.terminal {
                blocked_tasks.insert(record.task_id);
            }
            continue;
        };
        // After the read above: every frame published before a record that
        // was just read is queued by now.
        send_waiting_live(socket, live).await?;
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
