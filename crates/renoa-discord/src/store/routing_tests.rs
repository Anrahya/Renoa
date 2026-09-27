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
fn reassignment_preserves_queued_targets_and_starts_a_separate_conversation() {
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
    store.recover().unwrap();
    assert_eq!(store.next_queued().unwrap().unwrap().agent_id, original);
    store.mark_running("101").unwrap();
    store
        .mark_ready("101", "answer", &["answer".into()])
        .unwrap();
    let after = store.next_queued().unwrap().unwrap();
    assert_eq!(after.agent_id, child);
    assert_ne!(before.session_id, after.session_id);
    assert_eq!(
        store
            .enqueue(&snow("101"), &snow("202"), &snow("20"), b"first", "first")
            .unwrap(),
        Enqueue::Duplicate
    );
    drop(store);
    let reopened = SurfaceStore::open(files.path()).unwrap();
    reopened.recover().unwrap();
    assert_eq!(reopened.next_queued().unwrap().unwrap().agent_id, child);
    assert_eq!(reopened.bindings().unwrap()[0].revision, 2);
    let mut changed = second;
    changed.agent_id = original;
    assert!(reopened.bind_channel(&changed, "desk").is_err());
}

#[test]
fn schema_one_migrates_admitted_targets_without_changing_request_or_session_identity() {
    let files = tempfile::tempdir().unwrap();
    let database = files.path().join("surface.sqlite3");
    let original = Uuid::new_v4().to_string();
    let session = Uuid::new_v4().to_string();
    let request = Uuid::new_v4().to_string();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!("tests/schema_v1.sql"))
        .unwrap();
    connection.execute("INSERT INTO identity(singleton,guild_id,operator_user_id,agent_id) VALUES (1, '10', '20', ?1)", [&original]).unwrap();
    connection
        .execute("INSERT INTO messages VALUES ('101','202','20',x'01',0)", [])
        .unwrap();
    connection
        .execute("INSERT INTO conversations VALUES ('202',?1)", [&session])
        .unwrap();
    connection
        .execute(
            "INSERT INTO turns VALUES ('101',?1,?2,'task',NULL,'queued')",
            rusqlite::params![session, request],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    drop(connection);
    let upgraded = schema::open(&database).unwrap();
    let fields: (String, String, String) = upgraded
        .query_row(
            "SELECT session_id, request_id, agent_id FROM turns",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(fields, (session, request, original));
    assert_eq!(
        upgraded
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(upgraded);
    schema::open(&database).unwrap();
}
