use super::*;

async fn result(f: &Fixture, sequence: i64, text: &str) -> Uuid {
    let run = RoutineRun {
        sequence,
        id: Uuid::new_v4(),
        routine_id: Uuid::new_v4(),
        agent_id: f.worker.agent_id,
        session_id: Uuid::new_v4(),
        due_ms: sequence,
        admitted_at_ms: sequence,
        prompt: "test automation".to_owned(),
        output: Some(text.to_owned()),
    };
    let id = run.id;
    f.worker
        .store
        .admit_routine_result(run)
        .await
        .expect("project");
    let delivery = f
        .worker
        .store
        .next_routine_delivery()
        .await
        .expect("delivery")
        .expect("ready");
    f.worker
        .store
        .claim_routine_delivery(delivery.run_id.clone(), delivery.chunk)
        .await
        .expect("claim");
    f.worker
        .store
        .finish_routine_delivery(
            delivery.run_id,
            delivery.chunk,
            crate::store::DeliveryState::Sent,
            Some(sequence.to_string()),
            None,
        )
        .await
        .expect("sent");
    id
}
async fn bind(f: &Fixture) {
    let id = f.worker.agent_id.to_string();
    f.worker.store.run(move|db|{db.execute("INSERT INTO bot_channels(agent_id,name,channel_id,state) VALUES(?1,'digest','D1','ready')",[id])?;Ok(())}).await.expect("bind");
}

#[tokio::test]
async fn delivered_result_enters_followup_context_once_and_replay_keeps_its_snapshot() {
    let mut f = Fixture::new().await;
    bind(&f).await;
    let first = result(&f, 1, "status=green").await;
    f.admit("E1", "1.000001", "Explain the automation result.")
        .await;
    let work = f
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("work");
    let snapshot = work.surface_context.clone().expect("context");
    assert!(snapshot.contains("status=green"));
    assert!(snapshot.contains(&first.to_string()));
    result(&f, 2, "status=blue").await;
    f.admit("E1", "1.000001", "Explain the automation result.")
        .await;
    let replay = f
        .worker
        .store
        .next_work()
        .await
        .expect("retry")
        .expect("same work");
    assert_eq!(replay.surface_context, Some(snapshot));
    f.worker.session = None;
    f.worker.execute(replay).await.expect("model sees result");
    while let Some(delivery) = f.worker.store.next_delivery().await.expect("outbox") {
        f.worker.deliver(delivery).await.expect("deliver");
    }
    f.admit("E2", "2.000001", "What changed?").await;
    let next = f
        .worker
        .store
        .next_work()
        .await
        .expect("next")
        .expect("work");
    let context = next.surface_context.as_ref().expect("new context");
    assert!(!context.contains("status=green"));
    assert!(context.contains("status=blue"));
    f.worker.execute(next).await.expect("followup");
    let captured =
        std::fs::read_to_string(f.bridge.with_file_name("model-requests")).expect("model boundary");
    let requests: Vec<Value> = captured
        .lines()
        .map(|s| serde_json::from_str(s).expect("json"))
        .collect();
    assert!(
        requests[0]["messages"][0]["content"][1]["text"]
            .as_str()
            .expect("context")
            .contains("status=green")
    );
    assert_eq!(requests[0]["system_prompt"], requests[1]["system_prompt"]);
    assert_eq!(requests[0]["messages"][0], requests[1]["messages"][0]);
    f.stop().await;
}

#[tokio::test]
async fn fresh_sessions_recover_visible_results_but_never_other_agents_or_unsent_results() {
    let f = Fixture::new().await;
    bind(&f).await;
    result(&f, 1, "visible output").await;
    let actor = f.worker.agent_id.to_string();
    f.worker.store.run(move|db|{
        db.execute("INSERT INTO routine_deliveries(run_id,chunk,agent_id,channel,text,state) VALUES('other',0,'another-agent','D1','private output','sent')",[])?;
        db.execute("INSERT INTO routine_deliveries(run_id,chunk,agent_id,channel,text,state) VALUES('unsent',0,?1,'D1','unsent output','unknown')",[actor])?;
        Ok(())
    }).await.expect("other results");
    f.admit("E1", "1.000001", "Follow up").await;
    let work = f
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("work");
    let context = work.surface_context.expect("context");
    assert!(context.contains("visible output"));
    assert!(!context.contains("private output"));
    assert!(!context.contains("unsent output"));
    f.admit("E2", "2.000001", "!new").await;
    f.admit("E3", "3.000001", "What did the automation say?")
        .await;
    let snapshot: String = f
        .worker
        .store
        .run(|db| {
            Ok(db.query_row(
                "SELECT surface_context FROM requests WHERE message_ts='3.000001'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .expect("fresh session snapshot");
    assert!(snapshot.contains("visible output"));
    f.stop().await;
}

#[tokio::test]
async fn a_cancelled_turn_does_not_consume_result_context() {
    let f = Fixture::new().await;
    bind(&f).await;
    result(&f, 1, "retained output").await;
    f.admit("E1", "1.000001", "Read that result").await;
    f.admit("E2", "2.000001", "!cancel").await;
    f.worker
        .store
        .run(|db| {
            db.execute(
                "UPDATE requests SET state='done' WHERE cancel_requested=1",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("cancelled before execution");
    f.admit("E3", "3.000001", "Try again").await;
    let snapshot: String = f
        .worker
        .store
        .run(|db| {
            Ok(db.query_row(
                "SELECT surface_context FROM requests WHERE message_ts='3.000001'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .expect("snapshot");
    assert!(snapshot.contains("retained output"));
    f.stop().await;
}
