use std::path::PathBuf;

use renoa_control::TaskId;
use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef,
};
use uuid::Uuid;

use super::{NodeStore, TargetBinding};

fn proposal(session: u128) -> TargetBinding {
    TargetBinding {
        target: "agent:alpha".to_owned(),
        agent_id: Uuid::from_u128(1),
        session_id: Uuid::from_u128(session),
        workspace: PathBuf::from("/srv/renoa/alpha"),
    }
}

fn command(id: u128) -> CommandEnvelope {
    CommandEnvelope {
        command_id: CommandId::from_uuid(Uuid::from_u128(id)),
        principal_id: PrincipalId::from_uuid(Uuid::from_u128(2)),
        surface: SurfaceRef::new("discord"),
        target: TargetRef::new("agent:alpha"),
        input: CommandInput::Text {
            text: "continue".to_owned(),
        },
    }
}

#[tokio::test]
async fn a_task_keeps_the_session_recorded_by_its_first_command() {
    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));

    let first = store
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit the first command");
    let redelivered = store
        .admit(task_id, command(10), proposal(101))
        .await
        .expect("a redelivered command converges on its admission");
    let second = store
        .admit(task_id, command(11), proposal(102))
        .await
        .expect("admit a later command on the same task");

    assert_eq!(first.binding.session_id, Uuid::from_u128(100));
    assert_eq!(redelivered, first);
    assert_eq!(second.binding.session_id, Uuid::from_u128(100));
}

#[tokio::test]
async fn a_task_cannot_move_to_another_agent() {
    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));
    store
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit the first command");

    let mut moved = proposal(101);
    moved.agent_id = Uuid::from_u128(9);
    assert!(store.admit(task_id, command(11), moved).await.is_err());
}

#[tokio::test]
async fn live_progress_is_recorded_once_and_the_final_projection_adds_only_the_rest() {
    use renoa_protocol::{ExecutionEventKind, ExecutionTerminal};

    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));
    let record = store
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit the command");
    let command_id = record.command.command_id;
    let message = |text: &str| ExecutionEventKind::AssistantMessage {
        text: text.to_owned(),
    };
    let started = ExecutionEventKind::ToolStarted {
        call_id: "read".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "proof.txt"}),
    };
    let finished = |output: &str| ExecutionEventKind::ToolFinished {
        call_id: "read".to_owned(),
        output: output.to_owned(),
        is_error: false,
    };

    for kind in [message("Reading."), started.clone(), finished("proof")] {
        assert!(
            store
                .append_progress(command_id, kind)
                .await
                .expect("record")
        );
    }
    assert!(
        !store
            .append_progress(command_id, started.clone())
            .await
            .expect("a re-driven tool start"),
        "a tool call is recorded once"
    );
    assert!(
        store
            .append_progress(command_id, ExecutionEventKind::TurnStarted)
            .await
            .is_err()
    );

    let terminal = ExecutionEventKind::ExecutionTerminated {
        terminal: ExecutionTerminal::Completed,
    };
    store
        .finish(
            command_id,
            vec![
                message("Reading."),
                started.clone(),
                finished("proof as history recorded it"),
                message("Done."),
                terminal.clone(),
            ],
        )
        .await
        .expect("finish");
    assert!(
        !store
            .append_progress(command_id, message("late"))
            .await
            .expect("progress after the terminal"),
        "a finished execution records nothing more"
    );

    let kinds = store
        .load_events_after(command_id, None)
        .await
        .expect("events")
        .into_iter()
        .map(|event| event.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            ExecutionEventKind::ExecutionStarted,
            message("Reading."),
            started,
            finished("proof"),
            message("Done."),
            terminal,
        ]
    );
}
