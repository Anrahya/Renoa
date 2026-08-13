use renoa_protocol::{CommandEnvelope, CommandId, CommandInput, ExecutionEvent};
use serde::{Deserialize, Serialize};

use crate::{
    DeviceCredentials, EnrollmentToken, ErrorCode, TaskEvent, TaskEventKind, TaskId, TaskSummary,
    operations::{NodeOperation, SurfaceOperation},
};

pub const JSON_WS_VERSION: u32 = 8;
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
    ListTasks {
        request_id: u64,
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
            Self::Enroll { .. } | Self::Authenticate { .. } | Self::AcknowledgeExecution { .. } => {
                true
            }
            Self::ListTasks { request_id } | Self::Submit { request_id, .. } => {
                interoperable(*request_id)
            }
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
            Self::Enroll { .. } | Self::Authenticate { .. } => None,
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
    TaskList {
        request_id: u64,
        tasks: Vec<TaskSummary>,
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
            | Self::ExecutionAcknowledged { .. }
            | Self::Execute { .. } => true,
            Self::TaskList { request_id, .. } | Self::CommandAccepted { request_id, .. } => {
                interoperable(*request_id)
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
