use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, LiveActivity, TargetRef,
};
use serde::{Deserialize, Serialize};

use renoa_protocol::PrincipalId;
use uuid::Uuid;

use crate::{
    ConnectionTicket, DeviceCredentials, DeviceId, DeviceSummary, EnrollmentToken, ErrorCode,
    HostMethod, NodeId, StopOutcome, TargetSummary, TaskEvent, TaskEventKind, TaskId, TaskSummary,
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
}

impl ClientMessage {
    pub(crate) fn has_interoperable_numbers(&self) -> bool {
        match self {
            Self::Enroll { .. }
            | Self::Authenticate { .. }
            | Self::AuthenticateTicket { .. }
            | Self::DeviceProof { .. }
            | Self::AdvertiseTargets { .. }
            | Self::AcknowledgeExecution { .. }
            | Self::AcknowledgeStop { .. }
            | Self::ServeHostControl
            | Self::CompleteHostCall { .. } => true,
            Self::ListTasks { request_id }
            | Self::ListTargets { request_id }
            | Self::ListDevices { request_id }
            | Self::RevokeDevice { request_id, .. }
            | Self::OpenTask { request_id, .. }
            | Self::Submit { request_id, .. }
            | Self::StopCommand { request_id, .. }
            | Self::DeleteTask { request_id, .. } => interoperable(*request_id),
            Self::Attach {
                request_id,
                after_sequence,
                ..
            } => interoperable(*request_id) && after_sequence.is_none_or(interoperable),
            Self::PublishExecutionEvents { events, .. } => {
                events.iter().all(execution_event_has_interoperable_numbers)
            }
            Self::PublishLiveActivity { activity, .. } => {
                live_activity_has_interoperable_numbers(activity)
            }
        }
    }

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
            Self::ListTasks { .. }
            | Self::ListTargets { .. }
            | Self::ListDevices { .. }
            | Self::RevokeDevice { .. }
            | Self::OpenTask { .. }
            | Self::Attach { .. }
            | Self::Submit { .. }
            | Self::StopCommand { .. }
            | Self::DeleteTask { .. } => return self.into_surface_operation(),
        };
        Some(JsonOperation::Node(operation))
    }

    fn into_surface_operation(self) -> Option<JsonOperation> {
        let (request_id, operation) = match self {
            Self::ListTasks { request_id } => (request_id, SurfaceOperation::ListTasks),
            Self::ListTargets { request_id } => (request_id, SurfaceOperation::ListTargets),
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
            Self::Enroll { .. }
            | Self::Authenticate { .. }
            | Self::AuthenticateTicket { .. }
            | Self::DeviceProof { .. }
            | Self::AdvertiseTargets { .. }
            | Self::AcknowledgeExecution { .. }
            | Self::PublishExecutionEvents { .. }
            | Self::AcknowledgeStop { .. }
            | Self::PublishLiveActivity { .. }
            | Self::ServeHostControl
            | Self::CompleteHostCall { .. } => return None,
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

impl ServerMessage {
    pub(crate) fn has_interoperable_numbers(&self) -> bool {
        match self {
            Self::Enrolled { .. }
            | Self::Authenticated { .. }
            | Self::DeviceChallenge { .. }
            | Self::ExecutionAcknowledged { .. }
            | Self::Execute { .. }
            | Self::Stop { .. }
            | Self::HostCall { .. } => true,
            Self::TaskList { request_id, .. }
            | Self::TargetList { request_id, .. }
            | Self::DeviceRevoked { request_id, .. }
            | Self::TaskOpened { request_id, .. }
            | Self::CommandAccepted { request_id, .. }
            | Self::StopAccepted { request_id, .. }
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
            Self::LiveActivity { activity, .. } => {
                live_activity_has_interoperable_numbers(activity)
            }
            Self::Error { request_id, .. } => request_id.is_none_or(interoperable),
        }
    }
}

fn live_activity_has_interoperable_numbers(activity: &LiveActivity) -> bool {
    match activity {
        LiveActivity::TextDelta { sequence, .. } => interoperable(*sequence),
        LiveActivity::TextReset => true,
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

#[cfg(test)]
mod tests {
    //! The directions the socket tests cannot reach: the coordinator never
    //! receives an unsafe number it would later send back, so the outbound
    //! checks are exercised here.

    use super::*;

    const LARGEST_SAFE: u64 = MAX_INTEROPERABLE_INTEGER;
    const SMALLEST_UNSAFE: u64 = MAX_INTEROPERABLE_INTEGER + 1;

    fn delta(sequence: u64) -> LiveActivity {
        LiveActivity::TextDelta {
            sequence,
            text: "text".to_owned(),
        }
    }

    #[test]
    fn a_live_frame_numbered_beyond_what_javascript_preserves_is_not_sent_or_accepted() {
        let published = |activity| ClientMessage::PublishLiveActivity {
            task_id: TaskId::new(),
            command_id: CommandId::new(),
            activity,
        };
        let relayed = |activity| ServerMessage::LiveActivity {
            task_id: TaskId::new(),
            command_id: CommandId::new(),
            activity,
        };

        assert!(published(delta(LARGEST_SAFE)).has_interoperable_numbers());
        assert!(!published(delta(SMALLEST_UNSAFE)).has_interoperable_numbers());
        assert!(published(LiveActivity::TextReset).has_interoperable_numbers());
        assert!(relayed(delta(LARGEST_SAFE)).has_interoperable_numbers());
        assert!(!relayed(delta(SMALLEST_UNSAFE)).has_interoperable_numbers());
        assert!(relayed(LiveActivity::TextReset).has_interoperable_numbers());
    }

    #[test]
    fn a_stop_request_id_beyond_what_javascript_preserves_is_not_sent_or_accepted() {
        let requested = |request_id| ClientMessage::StopCommand {
            request_id,
            task_id: TaskId::new(),
            command_id: CommandId::new(),
        };
        let accepted = |request_id| ServerMessage::StopAccepted {
            request_id,
            command_id: CommandId::new(),
            outcome: StopOutcome::Requested,
        };

        assert!(requested(LARGEST_SAFE).has_interoperable_numbers());
        assert!(!requested(SMALLEST_UNSAFE).has_interoperable_numbers());
        assert!(accepted(LARGEST_SAFE).has_interoperable_numbers());
        assert!(!accepted(SMALLEST_UNSAFE).has_interoperable_numbers());
    }
}
