use renoa_protocol::{CommandEnvelope, CommandId, CommandInput, ExecutionEvent, TargetRef};
use serde::{Deserialize, Serialize};

use crate::{
    ConnectionTicket, DeviceCredentials, DeviceId, DeviceSummary, EnrollmentToken, ErrorCode,
    NodeId, TargetSummary, TaskEvent, TaskEventKind, TaskId, TaskSummary,
    operations::{NodeOperation, SurfaceOperation},
};

pub const JSON_WS_VERSION: u32 = 14;
const MAX_INTEROPERABLE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_INTEROPERABLE_SIGNED_INTEGER: i64 = 9_007_199_254_740_991;

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
    },
    Submit {
        request_id: u64,
        task_id: TaskId,
        command_id: CommandId,
        input: CommandInput,
    },
    DeleteTask {
        request_id: u64,
        task_id: TaskId,
    },
    AdvertiseTargets {
        targets: Vec<TargetRef>,
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
}

impl ClientMessage {
    pub(crate) fn has_interoperable_numbers(&self) -> bool {
        match self {
            Self::Enroll { .. }
            | Self::Authenticate { .. }
            | Self::AuthenticateTicket { .. }
            | Self::DeviceProof { .. }
            | Self::AdvertiseTargets { .. }
            | Self::AcknowledgeExecution { .. } => true,
            Self::ListTasks { request_id }
            | Self::ListTargets { request_id }
            | Self::ListDevices { request_id }
            | Self::RevokeDevice { request_id, .. }
            | Self::OpenTask { request_id, .. }
            | Self::Submit { request_id, .. }
            | Self::DeleteTask { request_id, .. } => interoperable(*request_id),
            Self::Attach {
                request_id,
                after_sequence,
                ..
            } => interoperable(*request_id) && after_sequence.is_none_or(interoperable),
            Self::PublishExecutionEvents { events, .. } => {
                events.iter().all(execution_event_has_interoperable_numbers)
            }
        }
    }

    pub(crate) fn into_operation(self) -> Option<JsonOperation> {
        match self {
            Self::ListTasks { request_id } => Some(JsonOperation::Surface {
                request_id,
                operation: SurfaceOperation::ListTasks,
            }),
            Self::ListTargets { request_id } => Some(JsonOperation::Surface {
                request_id,
                operation: SurfaceOperation::ListTargets,
            }),
            Self::OpenTask {
                request_id,
                task_id,
                node_id,
                target,
            } => Some(JsonOperation::Surface {
                request_id,
                operation: SurfaceOperation::OpenTask {
                    task_id,
                    node_id,
                    target,
                },
            }),
            Self::Attach {
                request_id,
                task_id,
                after_sequence,
            } => Some(JsonOperation::Surface {
                request_id,
                operation: SurfaceOperation::Attach {
                    task_id,
                    after_sequence,
                },
            }),
            Self::Submit {
                request_id,
                task_id,
                command_id,
                input,
            } => Some(JsonOperation::Surface {
                request_id,
                operation: SurfaceOperation::Submit {
                    task_id,
                    command_id,
                    input,
                },
            }),
            Self::DeleteTask {
                request_id,
                task_id,
            } => Some(JsonOperation::Surface {
                request_id,
                operation: SurfaceOperation::DeleteTask { task_id },
            }),
            Self::ListDevices { request_id } => Some(JsonOperation::Surface {
                request_id,
                operation: SurfaceOperation::ListDevices,
            }),
            Self::RevokeDevice {
                request_id,
                device_id,
            } => Some(JsonOperation::Surface {
                request_id,
                operation: SurfaceOperation::RevokeDevice { device_id },
            }),
            Self::AdvertiseTargets { targets } => {
                Some(JsonOperation::Node(NodeOperation::AdvertiseTargets {
                    targets,
                }))
            }
            Self::AcknowledgeExecution {
                task_id,
                command_id,
            } => Some(JsonOperation::Node(NodeOperation::AcknowledgeExecution {
                task_id,
                command_id,
            })),
            Self::PublishExecutionEvents {
                task_id,
                command_id,
                events,
            } => Some(JsonOperation::Node(NodeOperation::PublishExecutionEvents {
                task_id,
                command_id,
                events,
            })),
            Self::Enroll { .. }
            | Self::Authenticate { .. }
            | Self::AuthenticateTicket { .. }
            | Self::DeviceProof { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JsonOperation {
    Surface {
        request_id: u64,
        operation: SurfaceOperation,
    },
    Node(NodeOperation),
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
    TaskDeleted {
        request_id: u64,
        task_id: TaskId,
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
    TaskEvent {
        event: TaskEvent,
    },
    Error {
        request_id: Option<u64>,
        code: ErrorCode,
        message: String,
    },
}

impl ServerMessage {
    pub(crate) fn has_interoperable_numbers(&self) -> bool {
        match self {
            Self::Enrolled { .. }
            | Self::Authenticated { .. }
            | Self::DeviceChallenge { .. }
            | Self::ExecutionAcknowledged { .. }
            | Self::Execute { .. } => true,
            Self::TaskList { request_id, .. }
            | Self::TargetList { request_id, .. }
            | Self::DeviceRevoked { request_id, .. }
            | Self::TaskOpened { request_id, .. }
            | Self::CommandAccepted { request_id, .. }
            | Self::TaskDeleted { request_id, .. } => interoperable(*request_id),
            Self::DeviceList {
                request_id,
                devices,
            } => {
                interoperable(*request_id)
                    && devices
                        .iter()
                        .all(|device| interoperable(device.created_at_ms))
            }
            Self::Attached {
                request_id,
                through_sequence,
                ..
            } => interoperable(*request_id) && through_sequence.is_none_or(interoperable),
            Self::ExecutionEventsAccepted {
                through_execution_sequence,
                ..
            } => interoperable(*through_execution_sequence),
            Self::TaskEvent { event } => {
                interoperable(event.sequence)
                    && match &event.kind {
                        TaskEventKind::CommandSubmitted { .. } => true,
                        TaskEventKind::ExecutionEvent { event, .. } => {
                            execution_event_has_interoperable_numbers(event)
                        }
                    }
            }
            Self::Error { request_id, .. } => request_id.is_none_or(interoperable),
        }
    }
}

const fn interoperable(value: u64) -> bool {
    value <= MAX_INTEROPERABLE_INTEGER
}

fn execution_event_has_interoperable_numbers(event: &ExecutionEvent) -> bool {
    interoperable(event.sequence)
        && (-MAX_INTEROPERABLE_SIGNED_INTEGER..=MAX_INTEROPERABLE_SIGNED_INTEGER)
            .contains(&event.recorded_at_ms)
}
