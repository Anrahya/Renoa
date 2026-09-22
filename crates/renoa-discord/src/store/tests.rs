use uuid::Uuid;

use super::{Admission, Enqueue, IncomingMessage, SurfaceStore};
use crate::snowflake::Snowflake;

fn snowflake(value: &str) -> Snowflake {
    Snowflake::parse(value).expect("test snowflake")
}

fn agent() -> Uuid {
    Uuid::parse_str("11111111-1111-4111-8111-111111111111").expect("agent id")
}

fn message(author: &str, canonical: &str) -> IncomingMessage {
    IncomingMessage {
        message_id: snowflake("101"),
        channel_id: snowflake("202"),
        author_id: snowflake(author),
        canonical: canonical.as_bytes().to_vec(),
    }
}

#[test]
fn same_message_is_admitted_once_and_a_changed_copy_conflicts() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = SurfaceStore::open(directory.path()).expect("open store");
    store
        .bind_identity(&snowflake("10"), &snowflake("20"), agent())
        .expect("bind identity");

    assert_eq!(
        store.admit(message("20", "hello")).expect("admit"),
        Admission::Accepted
    );
    assert_eq!(
        store.admit(message("20", "hello")).expect("duplicate"),
        Admission::Duplicate
    );
    let conflict = store.admit(message("20", "changed")).expect_err("conflict");
    assert!(
        conflict.to_string().contains("different content"),
        "{conflict}"
    );
}

#[test]
fn another_author_is_recorded() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = SurfaceStore::open(directory.path()).expect("open store");
    store
        .bind_identity(&snowflake("10"), &snowflake("20"), agent())
        .expect("bind identity");

    assert_eq!(
        store.admit(message("99", "hello")).expect("other author"),
        Admission::Accepted
    );
}

#[test]
fn a_different_guild_does_not_replace_the_stored_identity() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = SurfaceStore::open(directory.path()).expect("open store");
    store
        .bind_identity(&snowflake("10"), &snowflake("20"), agent())
        .expect("bind identity");
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
fn admission_before_identity_leaves_no_message() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = SurfaceStore::open(directory.path()).expect("open store");
    let error = store.admit(message("20", "hello")).expect_err("unbound");
    assert!(error.to_string().contains("not bound"), "{error}");
    store
        .bind_identity(&snowflake("10"), &snowflake("20"), agent())
        .expect("bind identity");
    assert_eq!(
        store
            .admit(message("20", "hello"))
            .expect("admit after bind"),
        Admission::Accepted
    );
}

#[test]
fn a_repeated_discord_message_does_not_queue_a_second_turn() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = SurfaceStore::open(directory.path()).expect("open store");
    store
        .bind_identity(&snowflake("10"), &snowflake("20"), agent())
        .expect("bind identity");
    let message_id = snowflake("101");
    let channel_id = snowflake("202");
    let author_id = snowflake("99");
    assert_eq!(
        store
            .enqueue(&message_id, &channel_id, &author_id, b"same", "hello")
            .expect("enqueue"),
        Enqueue::Fresh
    );
    assert_eq!(
        store
            .enqueue(&message_id, &channel_id, &author_id, b"same", "hello")
            .expect("duplicate"),
        Enqueue::Duplicate
    );
    assert_eq!(
        store
            .enqueue(&message_id, &channel_id, &author_id, b"different", "other")
            .expect("different bytes"),
        Enqueue::Duplicate
    );
    store.mark_running("101").expect("running");
    store.recover().expect("recover");
    assert!(store.next_queued().expect("requeued").is_some());
}

#[test]
fn an_unknown_reply_is_not_sent_again() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = SurfaceStore::open(directory.path()).expect("open store");
    store
        .bind_identity(&snowflake("10"), &snowflake("20"), agent())
        .expect("bind identity");
    store
        .enqueue(
            &snowflake("101"),
            &snowflake("202"),
            &snowflake("99"),
            b"same",
            "hello",
        )
        .expect("enqueue");
    store.mark_running("101").expect("running");
    store
        .mark_ready(
            "101",
            "answer-more",
            &["answer".to_owned(), "more".to_owned()],
        )
        .expect("ready");
    store.mark_sending("101", 0).expect("sending");
    store.recover().expect("recover");
    assert!(store.next_outbound().expect("outbound").is_none());
    assert_eq!(
        delivery_state(&store, "101", 1).expect("later page"),
        "failed"
    );
}

#[test]
fn a_rejected_first_page_does_not_leave_later_pages_pending() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = SurfaceStore::open(directory.path()).expect("open store");
    store
        .bind_identity(&snowflake("10"), &snowflake("20"), agent())
        .expect("bind identity");
    store
        .enqueue(
            &snowflake("101"),
            &snowflake("202"),
            &snowflake("99"),
            b"same",
            "hello",
        )
        .expect("enqueue");
    store.mark_running("101").expect("running");
    store
        .mark_ready(
            "101",
            "answer-more",
            &["answer".to_owned(), "more".to_owned()],
        )
        .expect("ready");
    store.mark_sending("101", 0).expect("sending");
    store.mark_failed("101", 0).expect("failed");
    assert!(store.next_outbound().expect("outbound").is_none());
    assert_eq!(
        delivery_state(&store, "101", 1).expect("later page"),
        "failed"
    );
}

fn delivery_state(
    store: &SurfaceStore,
    message_id: &str,
    chunk: i64,
) -> Result<String, crate::DiscordError> {
    store.access(|connection| {
        connection
            .query_row(
                "SELECT state FROM deliveries WHERE message_id = ?1 AND chunk = ?2",
                rusqlite::params![message_id, chunk],
                |row| row.get(0),
            )
            .map_err(crate::DiscordError::from)
    })
}
