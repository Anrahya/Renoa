//! What each client message asks the coordinator to do, and which role may
//! ask it.

use super::ClientMessage;
use crate::{
    agent_messages::OutgoingMessage,
    operations::{NodeOperation, PushOperation, SurfaceOperation},
    setup_notices::{NodeNoticeOperation, NoticeOperation},
};

impl ClientMessage {
    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive table maps every client message to its role"
    )]
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
            Self::RequestWake {
                request_id,
                wake_id,
                urgency,
            } => NodeOperation::RequestWake {
                request_id,
                wake_id,
                urgency,
            },
            Self::ListNoticeKeys { request_id } => NodeOperation::Notice {
                request_id,
                operation: NodeNoticeOperation::ListKeys,
            },
            Self::PutNotice { request_id, notice } => NodeOperation::Notice {
                request_id,
                operation: NodeNoticeOperation::Put(notice),
            },
            Self::WithdrawNotices {
                request_id,
                notice_ids,
            } => NodeOperation::Notice {
                request_id,
                operation: NodeNoticeOperation::Withdraw(notice_ids),
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
            | Self::CallHost { .. }
            | Self::ShareLink { .. }
            | Self::SetPushToken { .. }
            | Self::ClearPushToken { .. }
            | Self::SetNoticeKey { .. }
            | Self::ListNotices { .. }
            | Self::AcknowledgeNotice { .. } => return self.into_surface_operation(),
        };
        Some(JsonOperation::Node(operation))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive table maps every surface message to its operation"
    )]
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
            Self::ShareLink {
                request_id,
                share_id,
            } => (request_id, SurfaceOperation::ShareLink { share_id }),
            Self::SetPushToken {
                request_id,
                provider,
                token,
            } => (
                request_id,
                SurfaceOperation::Push(PushOperation::Set { provider, token }),
            ),
            Self::ClearPushToken { request_id } => {
                (request_id, SurfaceOperation::Push(PushOperation::Clear))
            }
            Self::SetNoticeKey {
                request_id,
                public_key,
            } => (
                request_id,
                SurfaceOperation::Notice(NoticeOperation::SetKey(public_key)),
            ),
            Self::ListNotices { request_id } => {
                (request_id, SurfaceOperation::Notice(NoticeOperation::List))
            }
            Self::AcknowledgeNotice {
                request_id,
                notice_id,
            } => (
                request_id,
                SurfaceOperation::Notice(NoticeOperation::Acknowledge(notice_id)),
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
            | Self::SendAgentMessage { .. }
            | Self::RequestWake { .. }
            | Self::ListNoticeKeys { .. }
            | Self::PutNotice { .. }
            | Self::WithdrawNotices { .. } => return None,
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
