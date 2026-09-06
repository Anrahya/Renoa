use super::*;

#[tokio::test]
async fn admitted_slack_request_executes_arcee_and_delivers_without_repeating_a_completed_turn() {
    let mut fixture = Fixture::new().await;
    let ack = fixture.admit("Ev1", "1.000001", "Do the real task.").await;
    assert_eq!(
        serde_json::from_str::<Value>(&ack).expect("ack JSON")["envelope_id"],
        "socket-Ev1"
    );
    let work = fixture
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("durable before ack");
    let session_id = work.session_id;
    fixture.worker.execute(work).await.expect("execute");
    let session = fixture.worker.session.as_ref().expect("kernel session");
    assert_eq!(session.agent_id(), fixture.worker.agent_id);
    assert_eq!(session.id(), session_id);
    assert!(session.history().expect("history").len() >= 2);
    // Represent process loss after the kernel completed but before the surface
    // recorded the result. Replay must consult the original kernel operation.
    fixture
        .worker
        .store
        .run(|connection| {
            connection.execute_batch(
                "DELETE FROM deliveries; UPDATE requests SET state='queued',result=NULL;",
            )?;
            Ok(())
        })
        .await
        .expect("crash boundary");
    fixture.worker.session = None;
    let retry = fixture
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("same request");
    fixture.worker.execute(retry).await.expect("replay");
    assert_eq!(
        std::fs::read_to_string(fixture.directory.path().join("model-calls")).expect("model calls"),
        "called\n"
    );
    let delivery = fixture
        .worker
        .store
        .next_delivery()
        .await
        .expect("outbox")
        .expect("final result");
    assert_eq!(delivery.text, "Arcee executed this Slack request.");
    assert!(delivery.ts.is_some());
    fixture.worker.shutdown.cancel();
    fixture
        .worker
        .deliver(delivery)
        .await
        .expect("deliver known message update");
    assert!(
        fixture
            .worker
            .store
            .next_delivery()
            .await
            .expect("empty outbox")
            .is_none()
    );
    assert!(
        fixture
            .sent
            .lock()
            .await
            .iter()
            .any(|message| message["text"] == "Arcee executed this Slack request.")
    );
    fixture.stop().await;
}

#[tokio::test]
async fn cancellation_before_start_does_not_require_model_dependencies() {
    let mut fixture = Fixture::new().await;
    fixture.admit("Ev1", "1.000001", "Do not run this.").await;
    fixture.admit("Ev2", "2.000001", "!cancel").await;
    std::fs::remove_file(&fixture.bridge).expect("remove executable dependency");
    let work = fixture
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("prompt");
    fixture
        .worker
        .execute(work)
        .await
        .expect("cancel before startup");
    let result = fixture
        .worker
        .store
        .next_delivery()
        .await
        .expect("outbox")
        .expect("cancelled result");
    assert_eq!(result.text, "Stopped.");
    assert!(fixture.worker.session.is_none());
    assert!(!fixture.directory.path().join("model-calls").exists());
    fixture.stop().await;
}

#[tokio::test]
async fn separate_threads_share_the_agent_but_keep_distinct_histories() {
    let mut fixture = Fixture::new().await;
    for (id, ts) in [("Ev1", "1.000001"), ("Ev2", "2.000001")] {
        let mut incoming = envelope(id, ts, "<@U2> hello");
        let payload = incoming.payload.as_mut().expect("payload");
        payload["event"]["type"] = json!("app_mention");
        payload["event"]["channel"] = json!("C1");
        payload["event"]["channel_type"] = json!("channel");
        fixture
            .receiver
            .admit_envelope(incoming)
            .await
            .expect("admit channel mention");
        let work = fixture
            .worker
            .store
            .next_work()
            .await
            .expect("queue")
            .expect("thread work");
        fixture.worker.execute(work).await.expect("execute thread");
        let history = fixture
            .worker
            .session
            .as_ref()
            .expect("session")
            .history()
            .expect("history");
        assert_eq!(
            history
                .iter()
                .filter(|entry| matches!(entry.message, renoa_agent::Message::User { .. }))
                .count(),
            1
        );
    }
    assert_eq!(
        fixture
            .worker
            .host
            .list_agents()
            .await
            .expect("roster")
            .len(),
        1
    );
    fixture.stop().await;
}
