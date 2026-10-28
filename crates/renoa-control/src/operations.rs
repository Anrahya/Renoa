use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, PrincipalId, SurfaceRef, TargetRef,
};
use serde::{Deserialize, Serialize};

use crate::{DeviceId, NodeId, TaskEventId, TaskId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum PeerIdentity {
    Surface {
        principal_id: PrincipalId,
        surface: SurfaceRef,
    },
    Node {
        node_id: NodeId,
    },
}

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

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SurfaceOperation {
    ListTasks,
    ListTargets,
    OpenTask {
        task_id: TaskId,
        node_id: NodeId,
        target: TargetRef,
    },
    Attach {
        task_id: TaskId,
        after_sequence: Option<u64>,
    },
    Submit {
        task_id: TaskId,
        command_id: CommandId,
        input: CommandInput,
    },
    DeleteTask {
        task_id: TaskId,
    },
    ListDevices,
    RevokeDevice {
        device_id: DeviceId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeOperation {
    AdvertiseTargets {
        targets: Vec<TargetRef>,
    },
    AcknowledgeExecution {
        task_id: TaskId,
        command_id: CommandId,
    },
    PublishExecutionEvents {
        task_id: TaskId,
        command_id: CommandId,
        events: Vec<ExecutionEvent>,
    },
}
