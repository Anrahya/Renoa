use super::*;
use crate::service::apply_immediate;
use crate::store::ImmediateAction;

async fn admit_cancel(store: &SurfaceStore, work: &WorkItem) -> ImmediateAction {
    store
        .admit(ParsedUpdate {
            update_id: 2,
            canonical: b"stop".to_vec(),
            topic: Some(work.topic),
            message_id: Some(12),
            kind: InboundKind::Cancel,
        })
        .await
        .expect("persist cancellation")
        .immediate
        .expect("targeted cancellation")
}

#[tokio::test]
async fn queued_cancellation_survives_recovery_and_does_not_poison_the_next_prompt() {
    let mut fixture = service_fixture().await;
    let work = admit_work(
        &fixture.store,
        1,
        InboundKind::Prompt("Never run this.".to_owned()),
    )
    .await;
    let action = admit_cancel(&fixture.store, &work).await;
    apply_immediate(&fixture.worker.active, action)
        .await
        .expect("signal idle worker");
    assert!(
        fixture
            .store
            .cancellation_requested(1)
            .await
            .expect("stored stop")
    );
    // A crash after marking the item running must redrive its original identity
    // and cancellation flag, even though no AgentSession has been assembled yet.
    fixture
        .store
        .mark_running(1)
        .await
        .expect("claim work before crash");
    assert_eq!(
        fixture
            .store
            .recover()
            .await
            .expect("recover worker")
            .requeued,
        1
    );
    let PendingAction::Execute(retry) = fixture
        .store
        .next_action()
        .await
        .expect("reload")
        .expect("work")
    else {
        panic!("expected recovered work");
    };
    assert_eq!(retry.request_id, work.request_id);
    fixture
        .worker
        .execute(retry)
        .await
        .expect("settle stopped work");
    assert!(!fixture.directory.path().join("stream-called").exists());
    let delivery = ready_delivery(&fixture.store).await;
    assert_eq!(delivery.text, "Stopped.");
    fixture
        .store
        .recover()
        .await
        .expect("recover settled result");
    assert_eq!(ready_delivery(&fixture.store).await.text, "Stopped.");
    finish_delivery(&fixture.store, delivery, 80).await;
    let PendingAction::Execute(cancel) = fixture
        .store
        .next_action()
        .await
        .expect("next")
        .expect("cancel")
    else {
        panic!("expected cancel acknowledgement");
    };
    fixture
        .worker
        .execute(cancel)
        .await
        .expect("settle stop acknowledgement");
    finish_delivery(&fixture.store, ready_delivery(&fixture.store).await, 81).await;
    let next = admit_work(
        &fixture.store,
        3,
        InboundKind::Prompt("Run this later task.".to_owned()),
    )
    .await;
    fixture
        .worker
        .execute(next)
        .await
        .expect("execute next prompt");
    assert_eq!(
        ready_delivery(&fixture.store).await.text,
        "Arcee completed the real path."
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn cancellation_after_flag_read_before_host_start_is_not_lost() {
    let mut fixture = service_fixture().await;
    let work = admit_work(
        &fixture.store,
        1,
        InboundKind::Prompt("Stop at startup.".to_owned()),
    )
    .await;
    let (started, at_startup) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    fixture.worker.before_execution = Some((started, released));
    let active = Arc::clone(&fixture.worker.active);
    let store = fixture.store.clone();
    let stop = async {
        at_startup.await.expect("worker passed stored flag check");
        let action = admit_cancel(&store, &work).await;
        apply_immediate(&active, action)
            .await
            .expect("signal request before Host startup");
        release.send(()).expect("resume worker");
    };
    let run = fixture.worker.run_agent(&work, Some("Stop at startup."));
    let (result, ()) = tokio::join!(run, stop);
    assert_eq!(result.expect("cancelled outcome"), "Stopped.");
    assert!(!fixture.directory.path().join("stream-called").exists());
    fixture.shutdown().await;
}

#[tokio::test]
async fn delayed_stop_does_not_target_a_different_request() {
    let mut fixture = service_fixture().await;
    let work = admit_work(
        &fixture.store,
        1,
        InboundKind::Prompt("Stop this.".to_owned()),
    )
    .await;
    let action = admit_cancel(&fixture.store, &work).await;
    let later = CancellationToken::new();
    fixture
        .worker
        .active
        .set(work.topic, work.draft_id + 1, later.clone())
        .await;
    apply_immediate(&fixture.worker.active, action)
        .await
        .expect("apply delayed stop");
    assert!(!later.is_cancelled());
    fixture
        .worker
        .execute(work)
        .await
        .expect("settle original cancellation");
    assert_eq!(ready_delivery(&fixture.store).await.text, "Stopped.");
    fixture.shutdown().await;
}
