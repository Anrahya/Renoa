use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, PrincipalId, SurfaceRef, TargetRef,
};
use serde::{Deserialize, Serialize};

use crate::{NodeId, TaskEventId, TaskId};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub event_id: TaskEventId,
    pub task_id: TaskId,
    pub sequence: u64,
    pub kind: TaskEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskEventKind {
    CommandSubmitted { command: CommandEnvelope },
    ExecutionEvent { event: ExecutionEvent },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SurfaceOperation {
    ListTasks,
    Attach {
        task_id: TaskId,
        after_sequence: Option<u64>,
    },
    Submit {
        task_id: TaskId,
        command_id: CommandId,
        input: CommandInput,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeOperation {
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
