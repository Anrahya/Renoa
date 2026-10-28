use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, LiveActivity, TargetCard, TargetRef,
};
use serde::{Deserialize, Serialize};

use renoa_protocol::PrincipalId;
use uuid::Uuid;

use crate::{
    ConnectionTicket, DeviceCredentials, DeviceId, DeviceSummary, EnrollmentToken, ErrorCode,
    HostMethod, NodeId, StopOutcome, TargetSummary, TaskEvent, TaskId, TaskSummary,
    agent_messages::OutgoingMessage,
    operations::{NodeOperation, SurfaceOperation},
};

pub const JSON_WS_VERSION: u32 = 14;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Enroll {
        version: u32,
        token: EnrollmentToken,
    },
    Authenticate {
        version: u32,
        credentials: DeviceCredentials,
    },
    AuthenticateTicket {
        version: u32,
        ticket: ConnectionTicket,
    },
    /// The signature a key-bound device answers its `device_challenge` with.
    DeviceProof {
        signature: String,
    },
    ListTasks {
        request_id: u64,
    },
    ListTargets {
        request_id: u64,
    },
    ListDevices {
        request_id: u64,
    },
    RevokeDevice {
        request_id: u64,
        device_id: DeviceId,
    },
    OpenTask {
        request_id: u64,
        task_id: TaskId,
        node_id: NodeId,
        target: TargetRef,
    },
    Attach {
        request_id: u64,
        task_id: TaskId,
        after_sequence: Option<u64>,
        /// Whether the surface also wants the task's ephemeral live activity.
        live: bool,
    },
    Submit {
        request_id: u64,
        task_id: TaskId,
        command_id: CommandId,
        input: CommandInput,
    },
    StopCommand {
        request_id: u64,
        task_id: TaskId,
        command_id: CommandId,
    },
    DeleteTask {
        request_id: u64,
        task_id: TaskId,
    },
    /// The owner's Host control call, from a signed-in app or browser.
    CallHost {
        request_id: u64,
        method: HostMethod,
        path: String,
        body: Option<String>,
    },
    AdvertiseTargets {
        targets: Vec<TargetCard>,
    },
    PublishExecutionEvents {
        task_id: TaskId,
        command_id: CommandId,
        events: Vec<ExecutionEvent>,
    },
    AcknowledgeExecution {
        task_id: TaskId,
        command_id: CommandId,
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
    /// The node answers Host control calls for the owner who enrolled it.
    ServeHostControl,
    CompleteHostCall {
        call_id: Uuid,
        status: u16,
        body: String,
    },
    SendAgentMessage {
        request_id: u64,
        from_command: CommandId,
        message_id: CommandId,
        node_id: NodeId,
        target: TargetRef,
        text: String,
    },
}

impl ClientMessage {
    pub(crate) fn into_operation(self) -> Option<JsonOperation> {
        let operation = match self {
            Self::AdvertiseTargets { targets } => NodeOperation::AdvertiseTargets { targets },
            Self::AcknowledgeExecution {
                task_id,
                command_id,
            } => NodeOperation::AcknowledgeExecution {
                task_id,
                command_id,
            },
            Self::PublishExecutionEvents {
                task_id,
                command_id,
                events,
            } => NodeOperation::PublishExecutionEvents {
                task_id,
                command_id,
                events,
            },
            Self::AcknowledgeStop {
                task_id,
                command_id,
            } => NodeOperation::AcknowledgeStop {
                task_id,
                command_id,
            },
            Self::PublishLiveActivity {
                task_id,
                command_id,
                activity,
            } => NodeOperation::PublishLiveActivity {
                task_id,
                command_id,
                activity,
            },
            Self::SendAgentMessage {
                request_id,
                from_command,
                message_id,
                node_id,
                target,
                text,
            } => NodeOperation::SendAgentMessage {
                request_id,
                message: OutgoingMessage {
                    from_command,
                    message_id,
                    to_node: node_id,
                    to_target: target,
                    text,
                },
            },
            Self::ServeHostControl => NodeOperation::ServeHostControl,
            Self::CompleteHostCall {
                call_id,
                status,
                body,
            } => NodeOperation::CompleteHostCall {
                call_id,
                status,
                body,
            },
            Self::Enroll { .. }
            | Self::Authenticate { .. }
            | Self::AuthenticateTicket { .. }
            | Self::DeviceProof { .. } => return None,
            Self::ListTargets { request_id } => {
                return Some(JsonOperation::ListTargets { request_id });
            }
            Self::ListTasks { .. }
            | Self::ListDevices { .. }
            | Self::RevokeDevice { .. }
            | Self::OpenTask { .. }
            | Self::Attach { .. }
            | Self::Submit { .. }
            | Self::StopCommand { .. }
            | Self::DeleteTask { .. }
            | Self::CallHost { .. } => return self.into_surface_operation(),
        };
        Some(JsonOperation::Node(operation))
    }

    fn into_surface_operation(self) -> Option<JsonOperation> {
        let (request_id, operation) = match self {
            Self::ListTasks { request_id } => (request_id, SurfaceOperation::ListTasks),
            Self::OpenTask {
                request_id,
                task_id,
                node_id,
                target,
            } => (
                request_id,
                SurfaceOperation::OpenTask {
                    task_id,
                    node_id,
                    target,
                },
            ),
            Self::Attach {
                request_id,
                task_id,
                after_sequence,
                live,
            } => (
                request_id,
                SurfaceOperation::Attach {
                    task_id,
                    after_sequence,
                    live,
                },
            ),
            Self::Submit {
                request_id,
                task_id,
                command_id,
                input,
            } => (
                request_id,
                SurfaceOperation::Submit {
                    task_id,
                    command_id,
                    input,
                },
            ),
            Self::StopCommand {
                request_id,
                task_id,
                command_id,
            } => (
                request_id,
                SurfaceOperation::StopCommand {
                    task_id,
                    command_id,
                },
            ),
            Self::DeleteTask {
                request_id,
                task_id,
            } => (request_id, SurfaceOperation::DeleteTask { task_id }),
            Self::ListDevices { request_id } => (request_id, SurfaceOperation::ListDevices),
            Self::RevokeDevice {
                request_id,
                device_id,
            } => (request_id, SurfaceOperation::RevokeDevice { device_id }),
            Self::CallHost {
                request_id,
                method,
                path,
                body,
            } => (
                request_id,
                SurfaceOperation::CallHost { method, path, body },
            ),
            Self::Enroll { .. }
            | Self::ListTargets { .. }
            | Self::Authenticate { .. }
            | Self::AuthenticateTicket { .. }
            | Self::DeviceProof { .. }
            | Self::AdvertiseTargets { .. }
            | Self::AcknowledgeExecution { .. }
            | Self::PublishExecutionEvents { .. }
            | Self::AcknowledgeStop { .. }
            | Self::PublishLiveActivity { .. }
            | Self::ServeHostControl
            | Self::CompleteHostCall { .. }
            | Self::SendAgentMessage { .. } => return None,
        };
        Some(JsonOperation::Surface {
            request_id,
            operation,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JsonOperation {
    Surface {
        request_id: u64,
        operation: SurfaceOperation,
    },
    Node(NodeOperation),
    /// Open to both roles: a surface lists its principal's targets, a node
    /// those of its owner.
    ListTargets {
        request_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Enrolled {
        version: u32,
        credentials: DeviceCredentials,
    },
    Authenticated {
        version: u32,
    },
    /// Sent instead of `authenticated` to a device that registered a key.
    DeviceChallenge {
        nonce: String,
    },
    TaskList {
        request_id: u64,
        tasks: Vec<TaskSummary>,
    },
    TargetList {
        request_id: u64,
        targets: Vec<TargetSummary>,
    },
    DeviceList {
        request_id: u64,
        devices: Vec<DeviceSummary>,
    },
    DeviceRevoked {
        request_id: u64,
        device_id: DeviceId,
    },
    TaskOpened {
        request_id: u64,
        task_id: TaskId,
    },
    Attached {
        request_id: u64,
        task_id: TaskId,
        through_sequence: Option<u64>,
    },
    CommandAccepted {
        request_id: u64,
        command_id: CommandId,
    },
    StopAccepted {
        request_id: u64,
        command_id: CommandId,
        outcome: StopOutcome,
    },
    TaskDeleted {
        request_id: u64,
        task_id: TaskId,
    },
    /// The Host's answer to a `call_host`, as its HTTP status and JSON body.
    HostAnswer {
        request_id: u64,
        status: u16,
        body: String,
    },
    ExecutionEventsAccepted {
        command_id: CommandId,
        through_execution_sequence: u64,
    },
    ExecutionAcknowledged {
        command_id: CommandId,
    },
    Execute {
        task_id: TaskId,
        command: CommandEnvelope,
    },
    Stop {
        task_id: TaskId,
        command_id: CommandId,
    },
    /// One owner call under `/v1/host`, for the node that serves the Host.
    HostCall {
        call_id: Uuid,
        principal_id: PrincipalId,
        method: HostMethod,
        path: String,
        body: Option<String>,
    },
    TaskEvent {
        event: TaskEvent,
    },
    LiveActivity {
        task_id: TaskId,
        command_id: CommandId,
        activity: LiveActivity,
    },
    Error {
        request_id: Option<u64>,
        code: ErrorCode,
        message: String,
    },
}

mod numbers;

#[cfg(test)]
mod tests;
