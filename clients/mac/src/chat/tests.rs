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
    transcript.apply(&events[0], 0);
    transcript.apply(&events[1], 0);
    assert!(transcript.working());
    for event in &events {
        transcript.apply(event, 0);
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
    transcript.apply(&said(TaskId::new(), 0, quiet), 0);
    transcript.apply(
        &executed(
            TaskId::new(),
            1,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Failed {
                    error: "model unavailable".to_owned(),
                },
            },
        ),
        0,
    );
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

#[test]
fn only_lines_new_when_they_arrive_come_in_and_the_owners_take_the_agents_time() {
    let task_id = TaskId::new();
    let asked = |sequence| said(task_id, sequence, CommandInput::from_text("Still there?"));
    let answered = |sequence| {
        executed(
            task_id,
            sequence,
            ExecutionEventKind::AssistantMessage {
                text: "Here.".to_owned(),
            },
        )
    };
    let mut transcript = Transcript::default();
    // An old turn, replayed an hour after it was said.
    let later = 60 * MINUTE_MS;
    assert_eq!(transcript.apply(&asked(0), later), None);
    assert!(
        transcript.coming_in(0),
        "an owner's line is new until its time says otherwise"
    );
    assert_eq!(transcript.apply(&answered(1), later), Some(0..1));
    assert!(
        !transcript.coming_in(0) && !transcript.coming_in(1),
        "a replay comes in still"
    );

    // Two lines sent while the agent was away both take its first time.
    transcript.apply(&asked(70), later);
    transcript.apply(&asked(71), later);
    assert_eq!(transcript.apply(&answered(72), 72 * MINUTE_MS), Some(2..4));
    assert!(
        (2..5).all(|index| transcript.coming_in(index)),
        "a new turn comes in"
    );
    assert_eq!(transcript.lines()[2].at_ms, Some(72 * MINUTE_MS));
    assert_eq!(
        transcript.apply(&answered(73), 73 * MINUTE_MS),
        None,
        "nothing left to time"
    );
}

#[test]
fn transcript_tracks_plain_activity_from_tools_and_live_frames_and_clears_it() {
    use renoa_protocol::{CommandId, ExecutionEvent, ExecutionEventId, ExecutionId, LiveActivity};
    use renoa_rcp_wire::{TaskEvent, TaskEventId, TaskEventKind};

    let task_id = TaskId::new();
    let command_id = CommandId::new();
    let other_command = CommandId::new();
    let execution_id = ExecutionId::new();
    let exec_for = |seq: u64, cmd: CommandId, kind: ExecutionEventKind| TaskEvent {
        event_id: TaskEventId::new(),
        task_id,
        sequence: seq,
        kind: TaskEventKind::ExecutionEvent {
            command_id: cmd,
            event: ExecutionEvent {
                event_id: ExecutionEventId::new(),
                execution_id,
                sequence: seq,
                recorded_at_ms: i64::try_from(seq).expect("seq") * MINUTE_MS,
                kind,
            },
        },
    };

    let mut transcript = Transcript::default();
    transcript.apply(&exec_for(0, command_id, ExecutionEventKind::TurnStarted), 0);
    assert!(transcript.working());
    assert_eq!(transcript.doing(), None);

    assert!(transcript.apply_live(
        command_id,
        &LiveActivity::Activity {
            text: "  \u{200B}Scanning \u{202E}Cargo workspace\u{E0020}  ".to_owned(),
        },
    ));
    assert_eq!(transcript.doing(), Some("Scanning Cargo workspace"));

    assert!(!transcript.apply_live(
        other_command,
        &LiveActivity::Activity {
            text: "Stale command".to_owned(),
        },
    ));
    assert_eq!(transcript.doing(), Some("Scanning Cargo workspace"));

    assert!(transcript.apply_live(
        command_id,
        &LiveActivity::Activity {
            text: "Bad\ncontrol\tchars".to_owned(),
        },
    ));
    assert_eq!(transcript.doing(), None);

    assert!(transcript.apply_live(
        command_id,
        &LiveActivity::Activity {
            text: "a".repeat(renoa_protocol::MAX_ACTIVITY_BYTES),
        },
    ));
    assert_eq!(
        transcript.doing(),
        Some("a".repeat(renoa_protocol::MAX_ACTIVITY_BYTES).as_str())
    );

    assert!(transcript.apply_live(
        command_id,
        &LiveActivity::Activity {
            text: "a".repeat(renoa_protocol::MAX_ACTIVITY_BYTES + 1),
        },
    ));
    assert_eq!(transcript.doing(), None);

    transcript.apply(
        &exec_for(
            1,
            command_id,
            ExecutionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "read_file".to_owned(),
                arguments: serde_json::json!({ "path": "Cargo.toml" }),
                activity: Some("Reading Cargo.toml".to_owned()),
            },
        ),
        MINUTE_MS,
    );
    assert_eq!(transcript.doing(), Some("Reading Cargo.toml"));

    transcript.apply(
        &exec_for(
            2,
            command_id,
            ExecutionEventKind::AssistantMessage {
                text: "Here it is.".to_owned(),
            },
        ),
        2 * MINUTE_MS,
    );
    assert_eq!(transcript.doing(), None);

    assert!(transcript.apply_live(
        command_id,
        &LiveActivity::Activity {
            text: "Still working".to_owned(),
        },
    ));
    assert!(transcript.apply_live(command_id, &LiveActivity::TextReset));
    assert_eq!(transcript.doing(), None);
    assert!(!transcript.apply_live(command_id, &LiveActivity::TextReset));

    assert!(transcript.apply_live(
        command_id,
        &LiveActivity::Activity {
            text: "Retrying step".to_owned(),
        },
    ));
    transcript.clear_live();
    assert_eq!(transcript.doing(), None);

    transcript.apply(
        &exec_for(
            3,
            command_id,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            },
        ),
        3 * MINUTE_MS,
    );
    assert!(!transcript.working());
    assert!(!transcript.apply_live(
        command_id,
        &LiveActivity::Activity {
            text: "Late activity".to_owned(),
        },
    ));
    assert_eq!(transcript.doing(), None);
}
