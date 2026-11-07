//! The records the frames carry: summaries, task events and the small enums
//! both ends name.

use renoa_protocol::{CommandEnvelope, CommandId, ExecutionEvent, SurfaceRef, TargetRef};
use serde::{Deserialize, Serialize};

use crate::{DeviceId, NodeId, TaskEventId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AuthenticationFailed,
    InvalidMessage,
    InvalidRole,
    NodeOffline,
    NotFound,
    Conflict,
    Internal,
    ReplayRequired,
    VersionMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub task_id: TaskId,
    pub target: TargetRef,
    pub status: TaskStatus,
    /// The task's last committed sequence, as `attached` reports it; `None`
    /// for an empty task. A woken app attaches only where this passed its
    /// cursor.
    pub through_sequence: Option<u64>,
}

/// What a task is doing, from its latest command. The coordinator's
/// `task_status` derives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// The latest command has not reached its terminal record.
    Working,
    /// There is no command, or the latest one completed or was cancelled.
    Idle,
    /// The latest command's execution failed.
    Failed,
}

/// What a stop request found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopOutcome {
    /// The stop is durably recorded for the command, whether this request
    /// recorded it or an earlier one did.
    Requested,
    /// The command's execution had already ended; nothing was recorded.
    AlreadyEnded,
}

/// One signed-in app of the requesting principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub device_id: DeviceId,
    pub surface: SurfaceRef,
    /// The name the app gave when it signed in, such as "Pixel 9".
    pub label: String,
    pub created_at_ms: u64,
    /// Whether the connection that asked is this device.
    pub current: bool,
}

/// One agent an online node owned by the requesting principal can execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSummary {
    pub node_id: NodeId,
    pub target: TargetRef,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub event_id: TaskEventId,
    pub task_id: TaskId,
    pub sequence: u64,
    pub kind: TaskEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum TaskEventKind {
    CommandSubmitted {
        command: CommandEnvelope,
    },
    ExecutionEvent {
        command_id: CommandId,
        event: ExecutionEvent,
    },
}

/// The methods of an owner's Host control call. The relay forwards no other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HostMethod {
    Get,
    Post,
    Put,
}

/// How loudly a node asks to wake its owner. There is no quiet value: what
/// can wait until the owner opens the app asks for no wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeUrgency {
    /// A wake like a reply's.
    Soon,
    /// A wake that rings.
    Now,
}
