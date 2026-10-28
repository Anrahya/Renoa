//! Ephemeral live activity: a node's in-progress text reaches the surfaces
//! attached to the task, and nothing else.
//!
//! Live activity is never stored and never acknowledged. It travels on a
//! broadcast channel apart from the journal's. A surface that falls behind it
//! skips ahead, and a connection whose outbound queue is full drops the frame
//! instead of waiting for room, so no frame is ever queued behind a surface
//! that stopped reading. Frames and journal records do share that queue. The
//! durable `assistant_message` record supersedes whatever live text a surface
//! showed.

use renoa_protocol::{CommandId, LiveActivity};
use tokio::sync::{broadcast, mpsc};

use crate::{NodeId, ServerMessage, TaskId, control_log, coordinator::CoordinatorState};

pub(crate) const LIVE_BROADCAST_CAPACITY: usize = 256;

/// One task's live channel and the only node allowed to publish on it.
pub(crate) struct LiveChannel {
    node_id: NodeId,
    sender: broadcast::Sender<LiveFrame>,
}

#[derive(Debug, Clone)]
pub(crate) struct LiveFrame {
    pub(crate) command_id: CommandId,
    pub(crate) activity: LiveActivity,
}

/// Subscribes to `task_id`'s live activity. `node_id` is the task's bound
/// node, which only an authorized attachment knows.
pub(crate) async fn subscribe(
    state: &CoordinatorState,
    task_id: TaskId,
    node_id: NodeId,
) -> broadcast::Receiver<LiveFrame> {
    state
        .live_channels
        .lock()
        .await
        .entry(task_id)
        .or_insert_with(|| LiveChannel {
            node_id,
            sender: broadcast::channel(LIVE_BROADCAST_CAPACITY).0,
        })
        .sender
        .subscribe()
}

/// Relays a frame a node published. With no live subscriber the frame goes
/// nowhere and nothing is allocated for the task; a frame from a node that is
/// not the task's is dropped.
pub(crate) async fn relay(
    state: &CoordinatorState,
    node_id: NodeId,
    task_id: TaskId,
    command_id: CommandId,
    activity: LiveActivity,
) {
    let channels = state.live_channels.lock().await;
    let Some(channel) = channels.get(&task_id) else {
        return;
    };
    if channel.node_id != node_id {
        control_log::event(
            "warn",
            "live_activity_from_unbound_node",
            &serde_json::json!({ "task_id": task_id, "node_id": node_id }),
        );
        return;
    }
    // No receiver left is the same as no subscriber.
    let _ = channel.sender.send(LiveFrame {
        command_id,
        activity,
    });
}

/// The next live frame, or never when the attachment did not ask for them.
pub(crate) async fn next_frame(
    frames: &mut Option<broadcast::Receiver<LiveFrame>>,
) -> Result<LiveFrame, broadcast::error::RecvError> {
    match frames {
        Some(frames) => frames.recv().await,
        None => std::future::pending().await,
    }
}

/// Hands a frame to a surface's connection without waiting for room: a full
/// queue drops it.
pub(crate) fn forward(outgoing: &mpsc::Sender<ServerMessage>, task_id: TaskId, frame: LiveFrame) {
    let LiveFrame {
        command_id,
        activity,
    } = frame;
    let _ = outgoing.try_send(ServerMessage::LiveActivity {
        task_id,
        command_id,
        activity,
    });
}

/// Forwards the frames already waiting for an attachment. The surface loop
/// calls it before it forwards a journal record: a node sends a frame before
/// the records it precedes and the coordinator relays them in that order, so
/// every frame published before the record is waiting by then. A frame
/// published right after the record may go first as well.
pub(crate) fn forward_waiting(
    frames: &mut Option<broadcast::Receiver<LiveFrame>>,
    outgoing: &mpsc::Sender<ServerMessage>,
    task_id: TaskId,
) {
    let Some(receiver) = frames.as_mut() else {
        return;
    };
    // Bounded by what the channel holds (and the lag report that may precede
    // it), so a node that keeps publishing cannot hold the record back.
    for _ in 0..=LIVE_BROADCAST_CAPACITY {
        match receiver.try_recv() {
            Ok(frame) => forward(outgoing, task_id, frame),
            Err(broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(broadcast::error::TryRecvError::Empty) => return,
            Err(broadcast::error::TryRecvError::Closed) => {
                *frames = None;
                return;
            }
        }
    }
}
