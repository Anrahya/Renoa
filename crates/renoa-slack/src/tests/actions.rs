use renoa_agent::{AgentEvent, AgentEventSink as _, ContentBlock, ToolCall, ToolOutput};

use super::*;
use crate::{actions::Actions, events::Progress};

fn setup_event(stage: &str) -> AgentEvent {
    let (status, key, url) = if stage == "credentials" {
        (
            "credential_required",
            "setup_url",
            "https://renoa.example/setup#secret-browser-key",
        )
    } else {
        (
            "authorization_required",
            "authorization_url",
            "https://provider.example/authorize?state=private-oauth-state",
        )
    };
    AgentEvent::ToolExecutionUpdate {
        call: ToolCall {
            id: "connect-1".to_owned(),
            name: "extension_manage".to_owned(),
            arguments: json!({}),
            thought_signature: None,
            namespace: None,
        },
        update: ToolOutput {
            content: vec![ContentBlock::text(
                json!({"status":status,key:url,"expires_at_ms":i64::MAX,"credential_kind":"oauth_client"}).to_string(),
            )],
            details: None,
            is_error: false,
        },
    }
}

#[tokio::test]
async fn setup_steps_are_separate_posts_deduplicated_and_survive_final_delivery() {
    let fixture = Fixture::new().await;
    fixture.admit("E1", "1.000001", "connect").await;
    let work = fixture
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("request");
    fixture
        .worker
        .store
        .mark_running(work.seq)
        .await
        .expect("running");
    let stop = CancellationToken::new();
    // Even an unconfirmed initial progress post must not hide setup actions.
    let (progress, task) = Progress::start(
        Arc::clone(&fixture.worker.api),
        work.topic.clone(),
        None,
        stop.clone(),
        Some(Actions {
            api: Arc::clone(&fixture.worker.api),
            store: fixture.worker.store.clone(),
            topic: work.topic,
            seq: work.seq,
            cancellation: CancellationToken::new(),
        }),
    );
    for stage in ["credentials", "authorization", "authorization"] {
        progress.emit(setup_event(stage)).await;
    }
    assert!(progress.action_error().await.is_none());
    stop.cancel();
    task.await.expect("progress joined");
    fixture
        .worker
        .store
        .finish(work.seq, "Connected.".to_owned())
        .await
        .expect("final result");
    let delivery = fixture
        .worker
        .store
        .next_delivery()
        .await
        .expect("outbox")
        .expect("final");
    fixture
        .worker
        .deliver(delivery)
        .await
        .expect("deliver final");
    let sent = fixture.sent.lock().await;
    assert_eq!(sent.len(), 3);
    assert!(sent.iter().all(|body| body.get("ts").is_none()));
    assert!(
        sent[0]["text"]
            .as_str()
            .expect("credentials")
            .contains("configure this connection")
    );
    assert!(
        sent[1]["text"]
            .as_str()
            .expect("authorization")
            .contains("Action needed: authorize access")
    );
    assert_eq!(sent[2]["text"], "Connected.");
    drop(sent);
    fixture
        .worker
        .store
        .run(|db| {
            let rows: i64 = db.query_row(
                "SELECT count(*) FROM setup_actions WHERE state='sent' AND slack_ts IS NOT NULL",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(rows, 2);
            Ok(())
        })
        .await
        .expect("durable receipts");
    for file in ["slack.sqlite3", "slack.sqlite3-wal"] {
        let bytes = std::fs::read(fixture.directory.path().join(file)).expect("database file");
        for secret in ["secret-browser-key", "private-oauth-state"] {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|part| part == secret.as_bytes())
            );
        }
    }
    fixture.stop().await;
}

#[tokio::test]
async fn setup_actions_in_channels_cancel_with_a_private_setup_instruction() {
    let fixture = Fixture::new().await;
    fixture.admit("E1", "1.000001", "connect").await;
    let work = fixture
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("request");
    fixture
        .worker
        .store
        .mark_running(work.seq)
        .await
        .expect("running");
    let stop = CancellationToken::new();
    let cancellation = CancellationToken::new();
    let topic = crate::ingress::Topic {
        channel: "C1".to_owned(),
        thread: "1.000001".to_owned(),
    };
    let (progress, task) = Progress::start(
        Arc::clone(&fixture.worker.api),
        topic.clone(),
        None,
        stop.clone(),
        Some(Actions {
            api: Arc::clone(&fixture.worker.api),
            store: fixture.worker.store.clone(),
            topic,
            seq: work.seq,
            cancellation: cancellation.clone(),
        }),
    );
    progress.emit(setup_event("credentials")).await;
    assert!(cancellation.is_cancelled());
    assert!(
        progress
            .action_error()
            .await
            .expect("visible failure")
            .contains("DM")
    );
    assert!(fixture.sent.lock().await.is_empty());
    stop.cancel();
    task.await.expect("progress joined");
    fixture.stop().await;
}
