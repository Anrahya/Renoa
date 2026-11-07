use renoa_protocol::{
    CommandId, CommandInput, ExecutionEvent, LiveActivity, PrincipalId, SurfaceRef, TargetCard,
    TargetRef,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{DeviceId, HostMethod, NodeId, TaskId, WakeUrgency, agent_messages::OutgoingMessage};

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

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SurfaceOperation {
    ListTasks,
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
    CallHost {
        method: HostMethod,
        path: String,
        body: Option<String>,
    },
    ShareLink {
        share_id: uuid::Uuid,
    },
    Push(PushOperation),
    Notice(crate::setup_notices::NoticeOperation),
}

/// A device's push-token registration: device state, not a task operation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PushOperation {
    Set {
        provider: String,
        token: crate::PushToken,
    },
    Clear,
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
        targets: Vec<TargetCard>,
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
    SendAgentMessage {
        request_id: u64,
        message: OutgoingMessage,
    },
    RequestWake {
        request_id: u64,
        wake_id: Uuid,
        urgency: WakeUrgency,
    },
    Notice {
        request_id: u64,
        operation: crate::setup_notices::NodeNoticeOperation,
    },
}
