//! What each client message asks the coordinator to do, and which role may
//! ask it.

use crate::{
    ClientMessage,
    agent_messages::OutgoingMessage,
    operations::{NodeOperation, PushOperation, SurfaceOperation},
    setup_notices::{NodeNoticeOperation, NoticeOperation},
};

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive table maps every client message to its role"
)]
pub(crate) fn into_operation(message: ClientMessage) -> Option<JsonOperation> {
    let operation = match message {
        ClientMessage::AdvertiseTargets { targets } => NodeOperation::AdvertiseTargets { targets },
        ClientMessage::AcknowledgeExecution {
            task_id,
            command_id,
        } => NodeOperation::AcknowledgeExecution {
            task_id,
            command_id,
        },
        ClientMessage::PublishExecutionEvents {
            task_id,
            command_id,
            events,
        } => NodeOperation::PublishExecutionEvents {
            task_id,
            command_id,
            events,
        },
        ClientMessage::AcknowledgeStop {
            task_id,
            command_id,
        } => NodeOperation::AcknowledgeStop {
            task_id,
            command_id,
        },
        ClientMessage::PublishLiveActivity {
            task_id,
            command_id,
            activity,
        } => NodeOperation::PublishLiveActivity {
            task_id,
            command_id,
            activity,
        },
        ClientMessage::SendAgentMessage {
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
        ClientMessage::RequestWake {
            request_id,
            wake_id,
            urgency,
        } => NodeOperation::RequestWake {
            request_id,
            wake_id,
            urgency,
        },
        ClientMessage::ListNoticeKeys { request_id } => NodeOperation::Notice {
            request_id,
            operation: NodeNoticeOperation::ListKeys,
        },
        ClientMessage::PutNotice { request_id, notice } => NodeOperation::Notice {
            request_id,
            operation: NodeNoticeOperation::Put(notice),
        },
        ClientMessage::WithdrawNotices {
            request_id,
            notice_ids,
        } => NodeOperation::Notice {
            request_id,
            operation: NodeNoticeOperation::Withdraw(notice_ids),
        },
        ClientMessage::ServeHostControl => NodeOperation::ServeHostControl,
        ClientMessage::CompleteHostCall {
            call_id,
            status,
            body,
        } => NodeOperation::CompleteHostCall {
            call_id,
            status,
            body,
        },
        ClientMessage::Enroll { .. }
        | ClientMessage::Authenticate { .. }
        | ClientMessage::AuthenticateTicket { .. }
        | ClientMessage::DeviceProof { .. } => return None,
        ClientMessage::ListTargets { request_id } => {
            return Some(JsonOperation::ListTargets { request_id });
        }
        ClientMessage::ListTasks { .. }
        | ClientMessage::ListDevices { .. }
        | ClientMessage::RevokeDevice { .. }
        | ClientMessage::OpenTask { .. }
        | ClientMessage::Attach { .. }
        | ClientMessage::Submit { .. }
        | ClientMessage::StopCommand { .. }
        | ClientMessage::DeleteTask { .. }
        | ClientMessage::CallHost { .. }
        | ClientMessage::ShareLink { .. }
        | ClientMessage::SetPushToken { .. }
        | ClientMessage::ClearPushToken { .. }
        | ClientMessage::SetNoticeKey { .. }
        | ClientMessage::ListNotices { .. }
        | ClientMessage::AcknowledgeNotice { .. } => return into_surface_operation(message),
    };
    Some(JsonOperation::Node(operation))
}

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive table maps every surface message to its operation"
)]
fn into_surface_operation(message: ClientMessage) -> Option<JsonOperation> {
    let (request_id, operation) = match message {
        ClientMessage::ListTasks { request_id } => (request_id, SurfaceOperation::ListTasks),
        ClientMessage::OpenTask {
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
        ClientMessage::Attach {
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
        ClientMessage::Submit {
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
        ClientMessage::StopCommand {
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
        ClientMessage::DeleteTask {
            request_id,
            task_id,
        } => (request_id, SurfaceOperation::DeleteTask { task_id }),
        ClientMessage::ListDevices { request_id } => (request_id, SurfaceOperation::ListDevices),
        ClientMessage::RevokeDevice {
            request_id,
            device_id,
        } => (request_id, SurfaceOperation::RevokeDevice { device_id }),
        ClientMessage::CallHost {
            request_id,
            method,
            path,
            body,
        } => (
            request_id,
            SurfaceOperation::CallHost { method, path, body },
        ),
        ClientMessage::ShareLink {
            request_id,
            share_id,
        } => (request_id, SurfaceOperation::ShareLink { share_id }),
        ClientMessage::SetPushToken {
            request_id,
            provider,
            token,
        } => (
            request_id,
            SurfaceOperation::Push(PushOperation::Set { provider, token }),
        ),
        ClientMessage::ClearPushToken { request_id } => {
            (request_id, SurfaceOperation::Push(PushOperation::Clear))
        }
        ClientMessage::SetNoticeKey {
            request_id,
            public_key,
        } => (
            request_id,
            SurfaceOperation::Notice(NoticeOperation::SetKey(public_key)),
        ),
        ClientMessage::ListNotices { request_id } => {
            (request_id, SurfaceOperation::Notice(NoticeOperation::List))
        }
        ClientMessage::AcknowledgeNotice {
            request_id,
            notice_id,
        } => (
            request_id,
            SurfaceOperation::Notice(NoticeOperation::Acknowledge(notice_id)),
        ),
        ClientMessage::Enroll { .. }
        | ClientMessage::ListTargets { .. }
        | ClientMessage::Authenticate { .. }
        | ClientMessage::AuthenticateTicket { .. }
        | ClientMessage::DeviceProof { .. }
        | ClientMessage::AdvertiseTargets { .. }
        | ClientMessage::AcknowledgeExecution { .. }
        | ClientMessage::PublishExecutionEvents { .. }
        | ClientMessage::AcknowledgeStop { .. }
        | ClientMessage::PublishLiveActivity { .. }
        | ClientMessage::ServeHostControl
        | ClientMessage::CompleteHostCall { .. }
        | ClientMessage::SendAgentMessage { .. }
        | ClientMessage::RequestWake { .. }
        | ClientMessage::ListNoticeKeys { .. }
        | ClientMessage::PutNotice { .. }
        | ClientMessage::WithdrawNotices { .. } => return None,
    };
    Some(JsonOperation::Surface {
        request_id,
        operation,
    })
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
