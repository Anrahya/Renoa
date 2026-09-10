use super::*;
use crate::{commands::Command, ingress::Incoming};

fn binding(workspace: &Path) -> Binding<'_> {
    Binding {
        host_id: Uuid::nil(),
        agent_id: Uuid::nil(),
        team: "T1",
        bot: "U2",
        user: "U3",
        workspace,
    }
}

fn incoming(event: &str, ts: &str, text: &str) -> Incoming {
    Incoming {
        event_id: event.to_owned(),
        topic: Topic {
            channel: "D1".to_owned(),
            thread: String::new(),
        },
        message_ts: ts.to_owned(),
        text: text.to_owned(),
        starts_conversation: true,
    }
}

#[tokio::test]
async fn admission_deduplicates_events_and_slack_double_subscriptions_across_restart() {
    let directory = tempfile::tempdir().expect("surface directory");
    let store = Store::open(directory.path(), &binding(directory.path())).expect("store");
    assert!(Store::open(directory.path(), &binding(directory.path())).is_err());
    assert!(
        store
            .admit(incoming("Ev1", "1.000001", "hello"), 1)
            .await
            .expect("admit")
            .queued
    );
    let work = store.next_work().await.expect("queue").expect("work");
    assert!(
        !store
            .admit(incoming("Ev1", "1.000001", "hello"), 2)
            .await
            .expect("retry")
            .queued
    );
    assert!(
        !store
            .admit(incoming("Ev2", "1.000001", "hello"), 2)
            .await
            .expect("second subscription")
            .queued
    );
    assert!(
        store
            .admit(incoming("Ev1", "1.000001", "changed"), 2)
            .await
            .is_err()
    );
    store.mark_running(work.seq).await.expect("running");
    drop(store);
    let store = Store::open(directory.path(), &binding(directory.path())).expect("reopen");
    let restored = store
        .next_work()
        .await
        .expect("queue")
        .expect("recovered work");
    assert_eq!(restored.request_id, work.request_id);
    assert_eq!(restored.session_id, work.session_id);
    assert_eq!(restored.observed_at_ms, 1);
}

#[tokio::test]
async fn cancel_targets_admitted_work_and_new_rotates_only_future_requests() {
    let directory = tempfile::tempdir().expect("directory");
    let store = Store::open(directory.path(), &binding(directory.path())).expect("store");
    store
        .admit(incoming("Ev1", "1.000001", "hello"), 1)
        .await
        .expect("prompt");
    let original = store.next_work().await.expect("queue").expect("work");
    let cancel = store
        .admit(incoming("Ev2", "2.000001", "!cancel"), 2)
        .await
        .expect("cancel");
    assert_eq!(cancel.cancel_target, Some(original.request_id));
    assert!(
        store
            .cancelled(original.request_id)
            .await
            .expect("durable cancellation")
    );
    store
        .admit(incoming("Ev3", "3.000001", "!new"), 3)
        .await
        .expect("new");
    store
        .admit(incoming("Ev4", "4.000001", "next"), 4)
        .await
        .expect("next prompt");
    let ids = store
        .run(|connection| {
            let mut query = connection.prepare("SELECT session_id FROM requests ORDER BY seq")?;
            Ok(query
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .expect("stored identities");
    assert_eq!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    assert_eq!(ids[2], ids[3]);
}

#[tokio::test]
async fn restart_does_not_repeat_unknown_posts_but_retries_known_message_updates() {
    let directory = tempfile::tempdir().expect("directory");
    let store = Store::open(directory.path(), &binding(directory.path())).expect("store");
    store
        .admit(incoming("Ev1", "1.000001", "hello"), 1)
        .await
        .expect("prompt");
    let work = store.next_work().await.expect("queue").expect("work");
    assert!(matches!(work.command, Command::Prompt(_)));
    store.mark_running(work.seq).await.expect("running");
    store
        .reply_state(work.seq, ReplyState::Known, Some("2.000001".to_owned()))
        .await
        .expect("receipt");
    let output = "🦀".repeat(crate::formatting::MESSAGE_CHARACTERS + 500);
    store
        .finish(work.seq, output.clone())
        .await
        .expect("durable result");
    for (chunk, ts) in [(0, Some("2.000001".to_owned())), (1, None)] {
        store
            .delivery_state(work.seq, chunk, DeliveryState::Sending, ts, None)
            .await
            .expect("delivery began");
    }
    drop(store);
    let store = Store::open(directory.path(), &binding(directory.path())).expect("restart");
    assert!(store.next_work().await.expect("no re-execution").is_none());
    let delivery = store
        .next_delivery()
        .await
        .expect("delivery")
        .expect("safe update retry");
    assert_eq!(delivery.chunk, 0);
    assert_eq!(delivery.ts.as_deref(), Some("2.000001"));
    store
        .delivery_state(
            delivery.seq,
            delivery.chunk,
            DeliveryState::Sent,
            None,
            None,
        )
        .await
        .expect("receipt");
    assert!(
        store
            .next_delivery()
            .await
            .expect("no post retry")
            .is_none()
    );
    assert_eq!(crate::formatting::chunks(&output).concat(), output);
}

#[tokio::test]
async fn uncertain_middle_chunk_blocks_the_remaining_suffix_across_restart() {
    let directory = tempfile::tempdir().expect("directory");
    let store = Store::open(directory.path(), &binding(directory.path())).expect("store");
    store
        .admit(incoming("Ev1", "1.000001", "long answer"), 1)
        .await
        .expect("prompt");
    let work = store.next_work().await.expect("queue").expect("request");
    store.mark_running(work.seq).await.expect("running");
    store
        .finish(
            work.seq,
            "x".repeat(crate::formatting::MESSAGE_CHARACTERS * 2 + 1000),
        )
        .await
        .expect("long result");
    store
        .delivery_state(
            work.seq,
            0,
            DeliveryState::Sent,
            Some("2.000001".to_owned()),
            None,
        )
        .await
        .expect("first chunk receipt");
    store
        .delivery_state(
            work.seq,
            1,
            DeliveryState::Unknown,
            None,
            Some("lost response".to_owned()),
        )
        .await
        .expect("uncertain second chunk");
    assert!(
        store
            .next_delivery()
            .await
            .expect("suffix fenced")
            .is_none()
    );
    drop(store);
    let store = Store::open(directory.path(), &binding(directory.path())).expect("restart");
    assert!(
        store
            .next_delivery()
            .await
            .expect("suffix still fenced")
            .is_none()
    );
    let state = store
        .run(move |connection| {
            Ok(connection.query_row(
                "SELECT state FROM deliveries WHERE request_seq=?1 AND chunk=2",
                [work.seq],
                |row| row.get::<_, String>(0),
            )?)
        })
        .await
        .expect("retained suffix");
    assert_eq!(state, "pending");
}

#[tokio::test]
async fn schema_one_upgrade_preserves_queued_operator_session_and_its_identity() {
    let directory = tempfile::tempdir().expect("directory");
    let store = Store::open(directory.path(), &binding(directory.path())).expect("store");
    store
        .admit(incoming("Ev1", "1.000001", "hello"), 1)
        .await
        .expect("admit");
    let original = store.next_work().await.expect("queue").expect("work");
    drop(store);
    let database = Connection::open(directory.path().join("slack.sqlite3")).expect("database");
    database
        .execute_batch("DROP TABLE routine_context_receipts; DROP TABLE bot_channel_labels; DROP TABLE routine_deliveries; DROP TABLE routine_delivery_cursor; DROP TABLE setup_actions; ALTER TABLE requests DROP COLUMN surface_context; DROP TABLE bot_channels; ALTER TABLE sessions DROP COLUMN agent_id; PRAGMA user_version=1;")
        .expect("legacy schema");
    drop(database);
    let store = Store::open(directory.path(), &binding(directory.path())).expect("migrated store");
    let restored = store
        .next_work()
        .await
        .expect("queue")
        .expect("restored work");
    assert_eq!(restored.session_id, original.session_id);
    assert_eq!(restored.request_id, original.request_id);
    assert_eq!(
        store
            .session_agent(restored.session_id)
            .await
            .expect("legacy target"),
        Uuid::nil()
    );
    store
        .admit_with_agent(
            incoming("Ev2", "2.000001", "!agent bot"),
            2,
            AgentSelection::Selected(Uuid::new_v4()),
        )
        .await
        .expect("select");
    assert_eq!(
        store
            .session_agent(original.session_id)
            .await
            .expect("old target"),
        Uuid::nil()
    );
}

#[tokio::test]
async fn schema_three_upgrade_preserves_legacy_prompt_content_and_snapshots_new_admissions() {
    let directory = tempfile::tempdir().expect("directory");
    let store = Store::open(directory.path(), &binding(directory.path())).expect("store");
    store
        .admit(incoming("E1", "1.000001", "legacy prompt"), 1)
        .await
        .expect("admit");
    let original = store.next_work().await.expect("queue").expect("work");
    drop(store);
    let db = Connection::open(directory.path().join("slack.sqlite3")).expect("database");
    db.execute_batch("DROP TABLE routine_context_receipts; DROP TABLE bot_channel_labels; DROP TABLE routine_deliveries; DROP TABLE routine_delivery_cursor; DROP TABLE setup_actions; ALTER TABLE requests DROP COLUMN surface_context; PRAGMA user_version=3;")
        .expect("old schema");
    drop(db);
    let store = Store::open(directory.path(), &binding(directory.path())).expect("upgrade");
    let restored = store.next_work().await.expect("queue").expect("work");
    assert_eq!(original.request_id, restored.request_id);
    assert_eq!(
        restored.prompt_content().expect("legacy content"),
        vec![renoa_agent::ContentBlock::text("legacy prompt")]
    );
    store.mark_running(restored.seq).await.expect("running");
    store
        .finish(restored.seq, "done".to_owned())
        .await
        .expect("finish");
    store
        .admit(incoming("E2", "2.000001", "new prompt"), 2)
        .await
        .expect("new admission");
    let current = store.next_work().await.expect("queue").expect("work");
    let content = current.prompt_content().expect("content");
    assert_eq!(content.len(), 2);
    assert_eq!(
        current.surface_context.as_deref(),
        Some(crate::surface_context::CONTEXT)
    );
}

#[tokio::test]
async fn setup_action_recovery_keeps_unknown_posts_uncertain_and_retries_only_known_rejections() {
    let directory = tempfile::tempdir().expect("directory");
    let store = Store::open(directory.path(), &binding(directory.path())).expect("store");
    store
        .admit(incoming("E1", "1.000001", "connect"), 1)
        .await
        .expect("admit");
    let work = store.next_work().await.expect("queue").expect("request");
    store.mark_running(work.seq).await.expect("running");
    assert!(matches!(
        store
            .claim_action(
                work.seq,
                "call".to_owned(),
                "authorization".to_owned(),
                b"digest".to_vec()
            )
            .await
            .expect("claim"),
        super::DeliveryState::Sending
    ));
    drop(store);
    let store = Store::open(directory.path(), &binding(directory.path())).expect("reopen");
    store.mark_running(work.seq).await.expect("replay");
    assert!(matches!(
        store
            .claim_action(
                work.seq,
                "call".to_owned(),
                "authorization".to_owned(),
                b"digest".to_vec()
            )
            .await
            .expect("claim"),
        super::DeliveryState::Unknown
    ));
    assert!(matches!(
        store
            .claim_action(
                work.seq,
                "next-call".to_owned(),
                "authorization".to_owned(),
                b"new-digest".to_vec()
            )
            .await
            .expect("new action"),
        super::DeliveryState::Sending
    ));
    store
        .action_state(
            work.seq,
            "next-call".to_owned(),
            "authorization".to_owned(),
            super::DeliveryState::Pending,
            None,
            None,
        )
        .await
        .expect("rate limited");
    assert!(matches!(
        store
            .claim_action(
                work.seq,
                "next-call".to_owned(),
                "authorization".to_owned(),
                b"new-digest".to_vec()
            )
            .await
            .expect("retry"),
        super::DeliveryState::Sending
    ));
    assert!(
        store
            .claim_action(
                work.seq,
                "next-call".to_owned(),
                "authorization".to_owned(),
                b"changed-digest".to_vec()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn interrupted_routine_delivery_is_not_blindly_posted_again_after_restart() {
    let directory = tempfile::tempdir().expect("directory");
    let store = Store::open(directory.path(), &binding(directory.path())).expect("store");
    let id = Uuid::new_v4().to_string();
    let inserted = id.clone();
    store.run(move|db|{db.execute("INSERT INTO routine_deliveries(run_id,chunk,agent_id,channel,text,state) VALUES(?1,0,'00000000-0000-0000-0000-000000000000','C1','digest','pending')",[inserted])?;Ok(())}).await.expect("outbox");
    store
        .claim_routine_delivery(id, 0)
        .await
        .expect("persist before post");
    drop(store);
    let restored = Store::open(directory.path(), &binding(directory.path())).expect("restart");
    assert!(
        restored
            .next_routine_delivery()
            .await
            .expect("uncertain delivery excluded")
            .is_none()
    );
    let state: String = restored
        .run(|db| Ok(db.query_row("SELECT state FROM routine_deliveries", [], |r| r.get(0))?))
        .await
        .expect("state");
    assert_eq!(state, "unknown");
}
