use renoa_control::{TaskEvent, TaskEventId, TaskEventKind, TaskId};
use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, ExecutionEventId, ExecutionEventKind,
    ExecutionId, ExecutionTerminal, PrincipalId, SurfaceRef, TargetRef,
};
use uuid::Uuid;

use super::{Applied, Enqueue, SurfaceStore};
use crate::snowflake::Snowflake;

fn snowflake(value: &str) -> Snowflake {
    Snowflake::parse(value).expect("test snowflake")
}

fn agent() -> Uuid {
    Uuid::parse_str("11111111-1111-4111-8111-111111111111").expect("agent id")
}

fn bound_store(directory: &tempfile::TempDir) -> SurfaceStore {
    let store = SurfaceStore::open(directory.path()).expect("open store");
    store
        .bind_identity(&snowflake("10"), &snowflake("20"), agent())
        .expect("bind identity");
    store
}

fn enqueue(store: &SurfaceStore, message_id: &str, prompt: &str) {
    assert_eq!(
        store
            .enqueue(
                &snowflake(message_id),
                &snowflake("202"),
                &snowflake("99"),
                message_id.as_bytes(),
                prompt,
            )
            .expect("enqueue"),
        Enqueue::Fresh
    );
}

fn command(command_id: Uuid, surface: &str, text: &str) -> CommandEnvelope {
    CommandEnvelope {
        command_id: CommandId::from_uuid(command_id),
        principal_id: PrincipalId::new(),
        surface: SurfaceRef::new(surface),
        target: TargetRef::new(format!("agent:{}", agent())),
        input: CommandInput::Text {
            text: text.to_owned(),
        },
    }
}

fn record(task_id: Uuid, sequence: u64, kind: TaskEventKind) -> TaskEvent {
    TaskEvent {
        event_id: TaskEventId::new(),
        task_id: TaskId::from_uuid(task_id),
        sequence,
        kind,
    }
}

fn execution(command_id: Uuid, sequence: u64, kind: ExecutionEventKind) -> TaskEventKind {
    TaskEventKind::ExecutionEvent {
        command_id: CommandId::from_uuid(command_id),
        event: ExecutionEvent {
            event_id: ExecutionEventId::new(),
            execution_id: ExecutionId::from_uuid(command_id),
            sequence,
            recorded_at_ms: 0,
            kind,
        },
    }
}

/// Submits the queued message and returns its task and command identities.
fn submit(store: &SurfaceStore) -> (Uuid, Uuid, String) {
    let turn = store.next_queued().expect("next queued").expect("turn");
    store.mark_opened(turn.task_id).expect("opened");
    store.mark_submitted(&turn.message_id).expect("submitted");
    (turn.task_id, turn.command_id, turn.message_id)
}

#[test]
fn same_message_is_queued_once_and_a_changed_copy_conflicts() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = bound_store(&directory);
    let (message, channel, author) = (snowflake("101"), snowflake("202"), snowflake("99"));
    assert_eq!(
        store
            .enqueue(&message, &channel, &author, b"same", "hello")
            .expect("enqueue"),
        Enqueue::Fresh
    );
    assert_eq!(
        store
            .enqueue(&message, &channel, &author, b"same", "hello")
            .expect("duplicate"),
        Enqueue::Duplicate
    );
    let conflict = store
        .enqueue(&message, &channel, &author, b"different", "other")
        .expect_err("different bytes");
    assert!(
        conflict.to_string().contains("different content"),
        "{conflict}"
    );
    let queued = store.next_queued().expect("queued").expect("turn");
    assert_eq!(
        (queued.message_id.as_str(), queued.prompt.as_str()),
        ("101", "hello")
    );
    assert!(!queued.opened);
}

#[test]
fn task_records_become_one_reply_to_the_discord_message_even_when_replayed() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = bound_store(&directory);
    enqueue(&store, "101", "Summarize today.");
    let (task_id, command_id, message_id) = submit(&store);
    let records = [
        record(
            task_id,
            0,
            TaskEventKind::CommandSubmitted {
                command: command(command_id, "discord", "Summarize today."),
            },
        ),
        record(
            task_id,
            1,
            execution(command_id, 0, ExecutionEventKind::ExecutionStarted),
        ),
        record(
            task_id,
            2,
            execution(
                command_id,
                1,
                ExecutionEventKind::AssistantMessage {
                    text: "Here is the summary.".to_owned(),
                },
            ),
        ),
        record(
            task_id,
            3,
            execution(
                command_id,
                2,
                ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Completed,
                },
            ),
        ),
    ];
    let ready = records
        .iter()
        .map(|record| store.apply_event(record).expect("apply record"))
        .collect::<Vec<_>>();
    assert_eq!(
        ready,
        vec![
            Applied::Recorded,
            Applied::Recorded,
            Applied::Recorded,
            Applied::ReplyReady
        ]
    );
    for record in &records {
        assert_eq!(
            store.apply_event(record).expect("replayed record"),
            Applied::Stale
        );
    }
    let target = store
        .progress_target(&command_id.to_string())
        .expect("progress target")
        .expect("known command");
    assert_eq!(target.channel_id, "202");
    assert_eq!(target.reply_to.as_deref(), Some(message_id.as_str()));

    assert_eq!(
        store.opened_tasks().expect("opened tasks"),
        vec![(task_id, Some(3))]
    );
    let reply = store.next_outbound().expect("outbound").expect("reply");
    assert_eq!(reply.body, "Here is the summary.");
    assert_eq!(reply.reply_to.as_deref(), Some(message_id.as_str()));
    assert_eq!(reply.channel_id, "202");
    store.mark_sending(&reply.command_id, 0).expect("sending");
    store
        .mark_sent(&reply.command_id, 0, &snowflake("303"))
        .expect("sent");
    assert!(store.next_outbound().expect("outbound").is_none());
    assert!(store.has_reply("303").expect("reply lookup"));
}

#[test]
fn a_command_from_another_surface_is_posted_with_its_origin_and_no_reply_reference() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = bound_store(&directory);
    enqueue(&store, "101", "Start a report.");
    let (task_id, _, _) = submit(&store);
    let foreign = Uuid::new_v4();
    store
        .apply_event(&record(
            task_id,
            0,
            TaskEventKind::CommandSubmitted {
                command: command(foreign, "control-room", "Add the chart."),
            },
        ))
        .expect("foreign command");
    store
        .apply_event(&record(
            task_id,
            1,
            execution(
                foreign,
                0,
                ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Failed {
                        error: "model unavailable".to_owned(),
                    },
                },
            ),
        ))
        .expect("failed execution");

    let reply = store.next_outbound().expect("outbound").expect("reply");
    assert_eq!(
        reply.body,
        "**control-room:** Add the chart.\n\nThe agent could not complete this turn: model unavailable"
    );
    assert_eq!(reply.reply_to, None);
}

#[test]
fn a_local_answer_replies_without_submitting_and_leaves_nothing_queued() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = bound_store(&directory);
    enqueue(&store, "101", "Are you there?");
    store
        .answer_locally("101", "The agent is offline.")
        .expect("answer");

    assert!(store.next_queued().expect("queued").is_none());
    let reply = store.next_outbound().expect("outbound").expect("reply");
    assert_eq!(reply.body, "The agent is offline.");
    assert_eq!(reply.reply_to.as_deref(), Some("101"));
}

#[test]
fn a_different_guild_does_not_replace_the_stored_identity() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = bound_store(&directory);
    let error = store
        .bind_identity(&snowflake("11"), &snowflake("20"), agent())
        .expect_err("guild change");
    assert!(error.to_string().contains("differs"), "{error}");
    let other_agent = Uuid::parse_str("22222222-2222-4222-8222-222222222222").expect("other agent");
    let error = store
        .bind_identity(&snowflake("10"), &snowflake("20"), other_agent)
        .expect_err("agent change");
    assert!(error.to_string().contains("differs"), "{error}");
    store
        .bind_identity(&snowflake("10"), &snowflake("20"), agent())
        .expect("same identity still binds");
}

#[test]
fn a_second_store_cannot_open_the_same_directory() {
    let directory = tempfile::tempdir().expect("temp directory");
    let _store = SurfaceStore::open(directory.path()).expect("first store");
    let error = SurfaceStore::open(directory.path()).expect_err("second store");
    assert!(error.to_string().contains("already owns"), "{error}");
}

#[test]
fn snowflakes_are_processed_in_numeric_order() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = bound_store(&directory);
    for message_id in ["100", "99"] {
        enqueue(&store, message_id, message_id);
    }

    assert_eq!(
        store
            .next_queued()
            .expect("next queued")
            .expect("queued")
            .message_id,
        "99"
    );
    for message_id in ["99", "100"] {
        store
            .answer_locally(message_id, message_id)
            .expect("answer");
    }
    assert_eq!(
        store
            .next_outbound()
            .expect("next outbound")
            .expect("outbound")
            .body,
        "99"
    );
}

#[test]
fn a_previous_unreleased_schema_is_refused_without_mutation() {
    let directory = tempfile::tempdir().expect("temp directory");
    let database = directory.path().join("legacy.sqlite3");
    let connection = rusqlite::Connection::open(&database).expect("legacy database");
    connection
        .execute_batch(
            "CREATE TABLE identity (singleton INTEGER PRIMARY KEY) STRICT;
             PRAGMA user_version = 2;",
        )
        .expect("legacy schema");
    drop(connection);

    let Err(error) = super::schema::open(&database) else {
        panic!("an unreleased schema must not be migrated");
    };
    assert!(error.to_string().contains("not supported"), "{error}");
    let connection = rusqlite::Connection::open(&database).expect("inspect legacy database");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("legacy version");
    assert_eq!(version, 2);
}

#[test]
fn an_unknown_reply_is_not_sent_again() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = bound_store(&directory);
    enqueue(&store, "101", "hello");
    store
        .answer_locally("101", &"x".repeat(2500))
        .expect("two-page answer");
    let first = store
        .next_outbound()
        .expect("outbound")
        .expect("first page");
    store.mark_sending(&first.command_id, 0).expect("sending");
    store.recover().expect("recover");
    assert!(store.next_outbound().expect("outbound").is_none());
    assert_eq!(delivery_state(&store, &first.command_id, 1), "failed");
}

#[test]
fn a_rejected_first_page_does_not_leave_later_pages_pending() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = bound_store(&directory);
    enqueue(&store, "101", "hello");
    store
        .answer_locally("101", &"x".repeat(2500))
        .expect("two-page answer");
    let first = store
        .next_outbound()
        .expect("outbound")
        .expect("first page");
    store.mark_sending(&first.command_id, 0).expect("sending");
    store.mark_failed(&first.command_id, 0).expect("failed");
    assert!(store.next_outbound().expect("outbound").is_none());
    assert_eq!(delivery_state(&store, &first.command_id, 1), "failed");
}

fn delivery_state(store: &SurfaceStore, command_id: &str, chunk: i64) -> String {
    store
        .access(|connection| {
            connection
                .query_row(
                    "SELECT state FROM deliveries WHERE command_id = ?1 AND chunk = ?2",
                    rusqlite::params![command_id, chunk],
                    |row| row.get(0),
                )
                .map_err(crate::DiscordError::from)
        })
        .expect("delivery state")
}

#[test]
fn a_recorded_progress_message_is_finished_once_its_command_is() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = bound_store(&directory);
    enqueue(&store, "101", "Summarize today.");
    let (task_id, command_id, _) = submit(&store);
    store
        .apply_event(&record(
            task_id,
            0,
            TaskEventKind::CommandSubmitted {
                command: command(command_id, "discord", "Summarize today."),
            },
        ))
        .expect("submitted");
    let command_id_text = command_id.to_string();
    store
        .record_progress_message(&command_id_text, "202", "900")
        .expect("record progress");
    let shown = store.shown_progress().expect("shown progress");
    assert_eq!(
        (shown.len(), shown[0].message_id.as_str(), shown[0].finished),
        (1, "900", false)
    );

    store
        .apply_event(&record(
            task_id,
            1,
            execution(
                command_id,
                0,
                ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Completed,
                },
            ),
        ))
        .expect("terminated");
    assert!(store.shown_progress().expect("shown progress")[0].finished);

    store
        .clear_progress_message(&command_id_text)
        .expect("clear progress");
    assert!(store.shown_progress().expect("shown progress").is_empty());
}
