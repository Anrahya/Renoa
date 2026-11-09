use renoa_protocol::{CommandInput, ExecutionEventKind, ExecutionTerminal};
use renoa_rcp_wire::{NodeId, TaskId};
use uuid::Uuid;

use super::{Line, Speaker, Transcript, chat_task_id};
use crate::test_events::{MINUTE_MS, executed, said};

#[test]
fn the_chat_is_the_one_the_phone_opens() {
    // The vector Android's ChatJournalTest pins for the same node and target.
    let node = NodeId::from_uuid(Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap());
    assert_eq!(
        chat_task_id(node, "renoa").as_uuid(),
        Uuid::parse_str("0ae02263-c173-3623-8a84-ed64ab6d1fbf").unwrap()
    );
    assert_ne!(chat_task_id(node, "renoa"), chat_task_id(node, "kaede"));
}

fn line(speaker: Speaker, text: &str, minute: i64) -> Line {
    Line {
        speaker,
        text: text.to_owned(),
        failed: false,
        at_ms: Some(minute * MINUTE_MS),
    }
}

#[test]
fn a_turn_reads_as_the_owner_then_the_agent_and_a_replay_adds_nothing() {
    let events = [
        said(
            TaskId::new(),
            0,
            CommandInput::from_text("What's for dinner?"),
        ),
        executed(TaskId::new(), 1, ExecutionEventKind::TurnStarted),
        executed(
            TaskId::new(),
            2,
            ExecutionEventKind::AssistantMessage {
                text: "Pasta.".to_owned(),
            },
        ),
        executed(
            TaskId::new(),
            3,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            },
        ),
    ];
    let mut transcript = Transcript::default();
    transcript.apply(&events[0]);
    transcript.apply(&events[1]);
    assert!(transcript.working());
    for event in &events {
        transcript.apply(event);
    }
    assert!(!transcript.working());
    assert_eq!(transcript.through(), Some(3));
    assert_eq!(
        transcript.lines(),
        [
            // The owner's line takes the time the agent began on it.
            line(Speaker::Owner, "What's for dinner?", 1),
            line(Speaker::Agent, "Pasta.", 2)
        ]
    );
}

#[test]
fn a_quiet_command_is_not_shown_and_a_failed_turn_says_so() {
    let CommandInput::Text {
        text,
        author,
        attachments,
        context,
        ..
    } = CommandInput::from_text("run the nightly check");
    let quiet = CommandInput::Text {
        text,
        context,
        author,
        quiet: true,
        attachments,
    };
    let mut transcript = Transcript::default();
    transcript.apply(&said(TaskId::new(), 0, quiet));
    transcript.apply(&executed(
        TaskId::new(),
        1,
        ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Failed {
                error: "model unavailable".to_owned(),
            },
        },
    ));
    let [failure] = transcript.lines() else {
        panic!("one line: {:?}", transcript.lines());
    };
    assert!(failure.failed);
    assert_eq!(failure.speaker, Speaker::Agent);
    assert!(
        !failure.text.contains("model unavailable"),
        "no plumbing reaches the owner"
    );
}
