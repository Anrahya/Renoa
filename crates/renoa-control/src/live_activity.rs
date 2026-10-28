//! Ephemeral live activity: a node's in-progress text reaches the surfaces
//! attached to the task, and nothing else.
//!
//! Live activity is never stored and never acknowledged. It travels on a
//! channel apart from the journal's, so a slow surface drops frames here and
//! never receives `replay_required` because of them. The durable
//! `assistant_message` record supersedes whatever live text a surface showed.

use renoa_protocol::{CommandId, LiveActivity};
use tokio::sync::broadcast;

use crate::{NodeId, TaskId, control_log, coordinator::CoordinatorState};

const LIVE_BROADCAST_CAPACITY: usize = 256;

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
