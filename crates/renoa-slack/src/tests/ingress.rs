use super::*;

#[tokio::test]
async fn only_the_operator_can_start_conversations_and_unmentioned_replies_require_a_binding() {
    let fixture = Fixture::new().await;
    let mut unauthorized = envelope("Ev1", "1.000001", "secret task");
    unauthorized.payload.as_mut().expect("payload")["event"]["user"] = json!("U9");
    assert!(
        fixture
            .receiver
            .admit_envelope(unauthorized)
            .await
            .expect("ignored event ack")
            .is_some()
    );
    assert!(
        fixture
            .worker
            .store
            .next_work()
            .await
            .expect("empty queue")
            .is_none()
    );
    let channel_event = |event: &str, ts: &str, text: &str, kind: &str| {
        let mut incoming = envelope(event, ts, text);
        let payload = incoming.payload.as_mut().expect("payload");
        payload["event"]["channel"] = json!("C1");
        payload["event"]["channel_type"] = json!("channel");
        payload["event"]["thread_ts"] = json!("1.000001");
        payload["event"]["type"] = json!(kind);
        incoming
    };
    fixture
        .receiver
        .admit_envelope(channel_event("Ev2", "2.000001", "unrelated", "message"))
        .await
        .expect("unbound thread");
    assert!(
        fixture
            .worker
            .store
            .next_work()
            .await
            .expect("empty queue")
            .is_none()
    );
    fixture
        .receiver
        .admit_envelope(channel_event(
            "Ev3",
            "1.000001",
            "<@U2> help",
            "app_mention",
        ))
        .await
        .expect("mention binds thread");
    fixture
        .receiver
        .admit_envelope(channel_event("Ev4", "3.000001", "continue", "message"))
        .await
        .expect("follow up admitted");
    let counts = fixture
        .worker
        .store
        .run(|connection| {
            Ok(connection.query_row(
                "SELECT count(*),count(DISTINCT session_id) FROM requests",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?)
        })
        .await
        .expect("admission facts");
    assert_eq!(counts, (2, 1));
    fixture.stop().await;
}

#[tokio::test]
async fn conflicting_retry_cannot_be_acknowledged() {
    let fixture = Fixture::new().await;
    fixture.admit("Ev1", "1.000001", "first").await;
    fixture.admit("Ev2", "2.000001", "second").await;
    assert!(
        fixture
            .receiver
            .admit_envelope(envelope("Ev2", "1.000001", "first"))
            .await
            .is_err()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn ignored_reply_stays_ignored_when_retried_after_the_thread_is_bound() {
    let fixture = Fixture::new().await;
    let channel = |id: &str, ts: &str, text: &str| {
        let mut event = envelope(id, ts, text);
        let payload = event.payload.as_mut().expect("payload");
        payload["event"]["channel"] = json!("C1");
        payload["event"]["channel_type"] = json!("channel");
        payload["event"]["thread_ts"] = json!("1.000001");
        event
    };
    fixture
        .receiver
        .admit_envelope(channel("Ev2", "2.000001", "do not retroactively run this"))
        .await
        .expect("ignore reply");
    fixture
        .receiver
        .admit_envelope(channel("Ev1", "1.000001", "<@U2> start"))
        .await
        .expect("bind thread");
    fixture
        .receiver
        .admit_envelope(channel("Ev2", "2.000001", "do not retroactively run this"))
        .await
        .expect("retry ignored reply");
    let count = fixture
        .worker
        .store
        .run(|connection| {
            Ok(
                connection.query_row("SELECT count(*) FROM requests", [], |row| {
                    row.get::<_, i64>(0)
                })?,
            )
        })
        .await
        .expect("queue");
    assert_eq!(count, 1);
    fixture.stop().await;
}

#[tokio::test]
async fn threaded_dm_replies_keep_the_dm_session_and_wrong_installations_fail_closed() {
    let fixture = Fixture::new().await;
    fixture.admit("Ev1", "1.000001", "hello").await;
    let mut threaded = envelope("Ev2", "2.000001", "continue");
    threaded.payload.as_mut().expect("payload")["event"]["thread_ts"] = json!("1.000001");
    fixture
        .receiver
        .admit_envelope(threaded)
        .await
        .expect("threaded DM");
    let count = fixture
        .worker
        .store
        .run(|connection| {
            Ok(connection.query_row(
                "SELECT count(DISTINCT session_id) FROM requests",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await
        .expect("sessions");
    assert_eq!(count, 1);
    let mut wrong = envelope("Ev3", "3.000001", "wrong bot");
    wrong.payload.as_mut().expect("payload")["authorizations"][0]["user_id"] = json!("U9");
    assert!(fixture.receiver.admit_envelope(wrong).await.is_err());
    fixture.stop().await;
}
