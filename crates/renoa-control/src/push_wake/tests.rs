use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, ExecutionEventId, ExecutionEventKind,
    ExecutionId, ExecutionTerminal, PrincipalId, SurfaceRef, TargetRef,
};

use super::{OWED, wake_reason};
use crate::TaskEventKind;

/// How many record shapes the wake rule tells apart.
const SHAPES: usize = 10;

/// Numbers every shape with an exhaustive match. A new variant does not
/// compile here until it is numbered, and once numbered the test fails until a
/// sample of it states whether it wakes.
fn shape(kind: &TaskEventKind) -> usize {
    match kind {
        TaskEventKind::CommandSubmitted { .. } => 0,
        TaskEventKind::ExecutionEvent { event, .. } => match &event.kind {
            ExecutionEventKind::ExecutionStarted => 1,
            ExecutionEventKind::TurnStarted => 2,
            ExecutionEventKind::AssistantMessage { text } if text.trim().is_empty() => 3,
            ExecutionEventKind::AssistantMessage { .. } => 4,
            ExecutionEventKind::ToolStarted { .. } => 5,
            ExecutionEventKind::ToolFinished { .. } => 6,
            ExecutionEventKind::ExecutionTerminated { terminal } => match terminal {
                ExecutionTerminal::Completed => 7,
                ExecutionTerminal::Failed { .. } => 8,
                ExecutionTerminal::Cancelled { .. } => 9,
            },
        },
    }
}

fn execution(kind: ExecutionEventKind) -> TaskEventKind {
    TaskEventKind::ExecutionEvent {
        command_id: CommandId::new(),
        event: ExecutionEvent {
            event_id: ExecutionEventId::new(),
            execution_id: ExecutionId::new(),
            sequence: 1,
            recorded_at_ms: 1,
            kind,
        },
    }
}

fn terminated(terminal: ExecutionTerminal) -> TaskEventKind {
    execution(ExecutionEventKind::ExecutionTerminated { terminal })
}

#[test]
fn only_a_reply_with_text_and_a_failed_run_wake_the_owner() {
    let samples = [
        (
            TaskEventKind::CommandSubmitted {
                command: CommandEnvelope {
                    command_id: CommandId::new(),
                    principal_id: PrincipalId::new(),
                    surface: SurfaceRef::new("android"),
                    target: TargetRef::new("agent:scout"),
                    input: CommandInput::from_text("what is new?"),
                    origin: None,
                },
            },
            None,
        ),
        (execution(ExecutionEventKind::ExecutionStarted), None),
        (execution(ExecutionEventKind::TurnStarted), None),
        (
            execution(ExecutionEventKind::AssistantMessage {
                text: String::new(),
            }),
            None,
        ),
        (
            execution(ExecutionEventKind::AssistantMessage {
                text: " \n\t".to_owned(),
            }),
            None,
        ),
        (
            execution(ExecutionEventKind::AssistantMessage {
                text: "Three new papers.".to_owned(),
            }),
            Some("assistant_message"),
        ),
        (
            execution(ExecutionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "search".to_owned(),
                arguments: serde_json::json!({}),
                activity: None,
            }),
            None,
        ),
        (
            execution(ExecutionEventKind::ToolFinished {
                call_id: "call-1".to_owned(),
                output: "results".to_owned(),
                is_error: false,
            }),
            None,
        ),
        (terminated(ExecutionTerminal::Completed), None),
        (
            terminated(ExecutionTerminal::Failed {
                error: "the model is unavailable".to_owned(),
            }),
            Some("execution_failed"),
        ),
        (
            terminated(ExecutionTerminal::Cancelled {
                reason: "stopped by the owner".to_owned(),
            }),
            None,
        ),
    ];
    let mut covered = [false; SHAPES];
    for (kind, expected) in &samples {
        covered[shape(kind)] = true;
        assert_eq!(wake_reason(kind), *expected, "{kind:?}");
    }
    assert!(
        covered.iter().all(|covered| *covered),
        "every record shape has a sample: {covered:?}"
    );
}

/// The tests outside the crate cannot name `OWED`, so each copies it; a copy
/// that drifts, or a count of rows that ignores it, fails here.
#[test]
fn every_test_counts_owed_wakes_with_the_one_predicate() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut copies = 0;
    let mut drifted = Vec::new();
    for entry in std::fs::read_dir(&directory).expect("read the tests directory") {
        let path = entry.expect("test entry").path();
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read a test file");
        for (number, line) in text.lines().enumerate() {
            if !line.contains("FROM push_wakes") || !line.contains("count(") {
                continue;
            }
            copies += 1;
            if !line.contains(&format!("FROM push_wakes WHERE {OWED}\"")) {
                drifted.push(format!("{}:{}", path.display(), number + 1));
            }
        }
    }
    assert!(drifted.is_empty(), "{drifted:?}");
    assert!(copies > 0, "no test counts owed wakes; delete this check");
}
