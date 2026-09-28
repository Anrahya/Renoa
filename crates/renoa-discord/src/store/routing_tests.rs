use super::*;
use crate::control::DiscordBindingRequest;

fn snow(value: &str) -> Snowflake {
    Snowflake::parse(value).unwrap()
}
fn request(agent: Uuid, revision: i64) -> DiscordBindingRequest {
    DiscordBindingRequest {
        operation_id: Uuid::new_v4(),
        channel_id: "202".into(),
        agent_id: agent,
        expected_revision: revision,
    }
}

#[test]
fn reassignment_keeps_queued_messages_on_their_task_and_starts_a_new_one() {
    let files = tempfile::tempdir().unwrap();
    let store = SurfaceStore::open(files.path()).unwrap();
    let original = Uuid::new_v4();
    let child = Uuid::new_v4();
    store
        .bind_identity(&snow("10"), &snow("20"), original)
        .unwrap();
    let first = request(original, 0);
    let receipt = store.bind_channel(&first, "desk").unwrap();
    store
        .enqueue(&snow("101"), &snow("202"), &snow("20"), b"first", "first")
        .unwrap();
    let before = store.next_queued().unwrap().unwrap();
    let second = request(child, 1);
    store.bind_channel(&second, "desk").unwrap();
    assert_eq!(store.bind_channel(&first, "renamed").unwrap(), receipt);
    assert!(store.bind_channel(&request(original, 1), "desk").is_err());
    store
        .enqueue(&snow("102"), &snow("202"), &snow("20"), b"second", "second")
        .unwrap();

    assert_eq!(store.next_queued().unwrap().unwrap().agent_id, original);
    store.answer_locally("101", "answer").unwrap();
    let after = store.next_queued().unwrap().unwrap();
    assert_eq!(after.agent_id, child);
    assert_ne!(before.task_id, after.task_id);
    assert_eq!(
        store
            .enqueue(&snow("101"), &snow("202"), &snow("20"), b"first", "first")
            .unwrap(),
        Enqueue::Duplicate
    );
    drop(store);
    let reopened = SurfaceStore::open(files.path()).unwrap();
    reopened.recover().unwrap();
    assert_eq!(
        reopened.next_queued().unwrap().unwrap().task_id,
        after.task_id
    );
    assert_eq!(reopened.bindings().unwrap()[0].revision, 2);
    let mut changed = second;
    changed.agent_id = original;
    assert!(reopened.bind_channel(&changed, "desk").is_err());
}

#[test]
fn schema_one_reaches_schema_four_keeping_its_identity_and_message_deduplication() {
    let files = tempfile::tempdir().unwrap();
    let database = files.path().join("surface.sqlite3");
    let original = Uuid::new_v4().to_string();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!("tests/schema_v1.sql"))
        .unwrap();
    connection.execute("INSERT INTO identity(singleton,guild_id,operator_user_id,agent_id) VALUES (1, '10', '20', ?1)", [&original]).unwrap();
    connection
        .execute("INSERT INTO messages VALUES ('101','202','20',x'01',0)", [])
        .unwrap();
    connection
        .execute(
            "INSERT INTO conversations VALUES ('202',?1)",
            [Uuid::new_v4().to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO turns VALUES ('101',?1,?2,'task',NULL,'queued')",
            rusqlite::params![Uuid::new_v4().to_string(), Uuid::new_v4().to_string()],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    drop(connection);

    let upgraded = schema::open(&database).unwrap();
    assert_eq!(
        upgraded
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        4
    );
    let agent: String = upgraded
        .query_row("SELECT agent_id FROM identity", [], |row| row.get(0))
        .unwrap();
    assert_eq!(agent, original);
    let counts: (i64, i64, i64) = upgraded
        .query_row(
            "SELECT (SELECT count(*) FROM messages), (SELECT count(*) FROM turns),
                    (SELECT count(*) FROM tasks)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        counts,
        (1, 0, 0),
        "in-process turns do not carry into RCP tasks"
    );
    drop(upgraded);
    schema::open(&database).unwrap();
}
