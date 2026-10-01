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
            context: Some("channel #desk (202)".to_owned()),
            author: renoa_protocol::Author::Principal,
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
async fn an_unfinished_command_keeps_its_surface_context_across_a_restart() {
    let files = tempfile::tempdir().expect("temporary directory");
    let path = files.path().join("node.sqlite");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));
    NodeStore::open(&path)
        .expect("open node ledger")
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit");

    let reopened = NodeStore::open(&path).expect("reopen node ledger");
    let unfinished = reopened.load_unfinished().await.expect("recover");
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].command, command(10));
    assert_eq!(
        unfinished[0].command.input.context(),
        Some("channel #desk (202)")
    );
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

/// Records one live report the way a running turn does.
async fn report(
    store: &NodeStore,
    ledger: &mut super::LiveLedger,
    command_id: CommandId,
    kind: renoa_protocol::ExecutionEventKind,
) -> bool {
    ledger.admit(&kind).expect("count the report")
        && store
            .append_progress(command_id, kind)
            .await
            .expect("record")
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

    let mut ledger = store.live_ledger(command_id).await.expect("ledger");
    for kind in [message("Reading."), started.clone(), finished("proof")] {
        assert!(report(&store, &mut ledger, command_id, kind).await);
    }
    let mut redriven = store.live_ledger(command_id).await.expect("ledger");
    assert!(
        !report(&store, &mut redriven, command_id, started.clone()).await,
        "a re-driven turn does not record its tool call again"
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

#[tokio::test]
async fn a_message_the_turn_repeats_is_recorded_live_in_its_place() {
    use renoa_protocol::{ExecutionEventKind, ExecutionTerminal};

    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));
    let command_id = store
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit the command")
        .command
        .command_id;
    let checking = ExecutionEventKind::AssistantMessage {
        text: "Checking.".to_owned(),
    };
    let tool = |call_id: &str| ExecutionEventKind::ToolStarted {
        call_id: call_id.to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({}),
    };
    let turn = vec![checking.clone(), tool("one"), checking.clone(), tool("two")];

    let mut ledger = store.live_ledger(command_id).await.expect("ledger");
    for kind in turn.clone() {
        assert!(report(&store, &mut ledger, command_id, kind).await);
    }
    let terminal = ExecutionEventKind::ExecutionTerminated {
        terminal: ExecutionTerminal::Completed,
    };
    let mut projection = turn.clone();
    projection.push(terminal.clone());
    store.finish(command_id, projection).await.expect("finish");

    let kinds = store
        .load_events_after(command_id, None)
        .await
        .expect("events")
        .into_iter()
        .map(|event| event.kind)
        .collect::<Vec<_>>();
    let mut expected = vec![ExecutionEventKind::ExecutionStarted];
    expected.extend(turn);
    expected.push(terminal);
    assert_eq!(kinds, expected);
}

#[tokio::test]
async fn a_task_is_named_for_deletion_only_once_its_executions_have_ended() {
    use renoa_protocol::{ExecutionEventKind, ExecutionTerminal};

    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));
    let command_id = CommandId::from_uuid(Uuid::from_u128(10));
    store
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit the command");
    assert!(
        store.task_session(task_id).await.is_err(),
        "an unfinished execution keeps its session"
    );

    store
        .finish(
            command_id,
            vec![ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            }],
        )
        .await
        .expect("finish");
    assert_eq!(
        store.task_session(task_id).await.expect("session"),
        Some((Uuid::from_u128(1), Uuid::from_u128(100)))
    );
    store.forget_task(task_id).await.expect("forget the task");
    assert_eq!(store.task_session(task_id).await.expect("session"), None);
    assert!(
        store
            .load_unfinished()
            .await
            .expect("unfinished")
            .is_empty()
    );
}
