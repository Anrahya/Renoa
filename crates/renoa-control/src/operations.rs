use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, LiveActivity, PrincipalId,
    SurfaceRef, TargetRef,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    pub status: TaskStatus,
}

/// What a task is doing, from its latest command. See `task_status`.
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
        live: bool,
    },
    Submit {
        task_id: TaskId,
        command_id: CommandId,
        input: CommandInput,
    },
    StopCommand {
        task_id: TaskId,
        command_id: CommandId,
    },
    DeleteTask {
        task_id: TaskId,
    },
    ListDevices,
    RevokeDevice {
        device_id: DeviceId,
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeOperation {
    ServeHostControl,
    CompleteHostCall {
        call_id: Uuid,
        status: u16,
        body: String,
    },
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
    AcknowledgeStop {
        task_id: TaskId,
        command_id: CommandId,
    },
    PublishLiveActivity {
        task_id: TaskId,
        command_id: CommandId,
        activity: LiveActivity,
    },
}
