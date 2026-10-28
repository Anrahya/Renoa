use std::path::PathBuf;

use renoa_control::TaskId;
use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef,
};
use uuid::Uuid;

use super::{NodeStore, StopRecord, TargetBinding};

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
        origin: None,
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

fn stop_rows(files: &tempfile::TempDir) -> i64 {
    rusqlite::Connection::open(files.path().join("node.sqlite"))
        .expect("open node ledger")
        .query_row("SELECT count(*) FROM host_node_stops", [], |row| row.get(0))
        .expect("count stops")
}

#[tokio::test]
async fn a_stop_is_recorded_once_even_before_its_command_is_admitted() {
    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));
    let command_id = CommandId::from_uuid(Uuid::from_u128(10));
    assert!(!store.stop_requested(command_id).await.expect("read"));

    for _ in 0..2 {
        assert_eq!(
            store
                .record_stop(task_id, command_id)
                .await
                .expect("record the stop"),
            StopRecord::Recorded
        );
    }
    store
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit the stopped command");

    assert!(store.stop_requested(command_id).await.expect("read"));
    assert!(
        !store
            .stop_requested(CommandId::from_uuid(Uuid::from_u128(11)))
            .await
            .expect("read another command")
    );
    assert_eq!(stop_rows(&files), 1);
}

#[tokio::test]
async fn a_stop_that_no_execution_can_honor_is_not_recorded() {
    use renoa_protocol::{ExecutionEventKind, ExecutionTerminal};

    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));
    let ended = CommandId::from_uuid(Uuid::from_u128(10));
    let running = CommandId::from_uuid(Uuid::from_u128(11));
    store
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit the first command");
    store
        .finish(
            ended,
            vec![ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            }],
        )
        .await
        .expect("finish the first command");
    store
        .admit(task_id, command(11), proposal(100))
        .await
        .expect("admit the second command");

    assert_eq!(
        store.record_stop(task_id, ended).await.expect("late stop"),
        StopRecord::AlreadyEnded
    );
    assert!(
        store
            .record_stop(TaskId::from_uuid(Uuid::from_u128(4)), running)
            .await
            .is_err(),
        "another task cannot stop this task's command"
    );

    assert!(!store.stop_requested(ended).await.expect("read"));
    assert!(!store.stop_requested(running).await.expect("read"));
    assert_eq!(stop_rows(&files), 0);
}

#[tokio::test]
async fn forgetting_a_task_forgets_its_stops_and_only_its_stops() {
    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let forgotten = TaskId::from_uuid(Uuid::from_u128(3));
    let kept = TaskId::from_uuid(Uuid::from_u128(4));
    let forgotten_command = CommandId::from_uuid(Uuid::from_u128(10));
    let kept_command = CommandId::from_uuid(Uuid::from_u128(11));
    store
        .record_stop(forgotten, forgotten_command)
        .await
        .expect("record a stop");
    store
        .record_stop(kept, kept_command)
        .await
        .expect("record another task's stop");

    store.forget_task(forgotten).await.expect("forget the task");

    assert!(!store.stop_requested(forgotten_command).await.expect("read"));
    assert!(store.stop_requested(kept_command).await.expect("read"));
}
