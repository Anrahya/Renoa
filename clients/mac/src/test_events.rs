//! Task events as a coordinator sends them, for tests.

use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, ExecutionEventId, ExecutionEventKind,
    ExecutionId, PrincipalId, SurfaceRef, TargetRef,
};
use renoa_rcp_wire::{TaskEvent, TaskEventId, TaskEventKind, TaskId};

pub const MINUTE_MS: i64 = 60_000;

fn task_event(task_id: TaskId, sequence: u64, kind: TaskEventKind) -> TaskEvent {
    TaskEvent {
        event_id: TaskEventId::new(),
        task_id,
        sequence,
        kind,
    }
}

pub fn said(task_id: TaskId, sequence: u64, input: CommandInput) -> TaskEvent {
    task_event(
        task_id,
        sequence,
        TaskEventKind::CommandSubmitted {
            command: CommandEnvelope {
                command_id: CommandId::new(),
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("mac"),
                target: TargetRef::new("renoa"),
                input,
                origin: None,
            },
        },
    )
}

/// An execution event recorded `sequence` minutes after the epoch.
pub fn executed(task_id: TaskId, sequence: u64, kind: ExecutionEventKind) -> TaskEvent {
    task_event(
        task_id,
        sequence,
        TaskEventKind::ExecutionEvent {
            command_id: CommandId::new(),
            event: ExecutionEvent {
                event_id: ExecutionEventId::new(),
                execution_id: ExecutionId::new(),
                sequence,
                recorded_at_ms: i64::try_from(sequence).expect("a small sequence") * MINUTE_MS,
                kind,
            },
        },
    )
}
