use std::path::Path;

use tempfile::tempdir;

use super::{PendingAction, SurfaceStore};
use crate::ingress::{InboundKind, ParsedUpdate, Topic};

const OWNER: i64 = 42;

fn update(update_id: i64, kind: InboundKind) -> ParsedUpdate {
    ParsedUpdate {
        update_id,
        canonical: format!("update-{update_id}").into_bytes(),
        topic: Some(Topic {
            chat_id: OWNER,
            thread_id: Some(7),
        }),
        message_id: Some(update_id + 100),
        kind,
    }
}

async fn store(root: &Path, workspace: &Path) -> SurfaceStore {
    let store = SurfaceStore::open(root).expect("open surface store");
    store
        .bind_identity(9, OWNER, workspace)
        .await
        .expect("bind surface identity");
    store
}

#[tokio::test]
async fn an_update_is_durable_before_offset_advance_and_exact_duplicates_are_noops() {
    let directory = tempdir().expect("temporary data root");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    let store = store(directory.path(), &workspace).await;

    let admitted = store
        .admit(update(10, InboundKind::Prompt("hello".to_owned())))
        .await
        .expect("admit prompt");
    assert!(admitted.queued);
    assert!(!admitted.duplicate);
    assert_eq!(store.next_update_id().await.expect("load offset"), 11);
    let PendingAction::Execute(original) = store
        .next_action()
        .await
        .expect("load work")
        .expect("queued prompt")
    else {
        panic!("prompt did not become work");
    };

    let duplicate = store
        .admit(update(10, InboundKind::Prompt("hello".to_owned())))
        .await
        .expect("admit exact duplicate");
    assert!(duplicate.duplicate);
    let PendingAction::Execute(replayed) = store
        .next_action()
        .await
        .expect("reload work")
        .expect("same queued prompt")
    else {
        panic!("duplicate changed work kind");
    };
    assert_eq!(replayed.request_id, original.request_id);
    assert_eq!(replayed.session_id, original.session_id);
    assert_eq!(replayed.observed_at_ms, original.observed_at_ms);

    let mut changed = update(10, InboundKind::Prompt("tampered".to_owned()));
    changed.canonical = b"different-update-10".to_vec();
    assert!(store.admit(changed).await.is_err());
}

#[tokio::test]
async fn new_rotates_only_the_topic_pointer_and_old_work_keeps_its_session() {
    let directory = tempdir().expect("temporary data root");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    let store = store(directory.path(), &workspace).await;

    store
        .admit(update(1, InboundKind::Prompt("first".to_owned())))
        .await
        .expect("admit first prompt");
    let first = execute(&store).await;
    settle(&store, first.update_id).await;

    store
        .admit(update(2, InboundKind::New))
        .await
        .expect("rotate conversation");
    let rotated = execute(&store).await;
    assert_ne!(rotated.session_id, first.session_id);
    settle(&store, rotated.update_id).await;

    store
        .admit(update(3, InboundKind::Prompt("second".to_owned())))
        .await
        .expect("admit second prompt");
    let second = execute(&store).await;
    assert_eq!(second.session_id, rotated.session_id);
    assert_ne!(second.session_id, first.session_id);
}

#[tokio::test]
async fn cancel_is_durable_even_when_it_arrives_before_the_worker_starts() {
    let directory = tempdir().expect("temporary data root");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    let store = store(directory.path(), &workspace).await;

    store
        .admit(update(1, InboundKind::Prompt("long task".to_owned())))
        .await
        .expect("admit prompt");
    store
        .admit(update(2, InboundKind::Cancel))
        .await
        .expect("admit cancel");
    let prompt = execute(&store).await;
    assert!(
        store
            .cancellation_requested(prompt.update_id)
            .await
            .expect("read cancellation")
    );
}

#[tokio::test]
async fn restart_requeues_execution_but_never_blindly_retries_final_delivery() {
    let directory = tempdir().expect("temporary data root");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    let store = store(directory.path(), &workspace).await;

    store
        .admit(update(1, InboundKind::Prompt("resume me".to_owned())))
        .await
        .expect("admit prompt");
    let work = execute(&store).await;
    store
        .mark_running(work.update_id)
        .await
        .expect("start work");
    let recovered = store.recover().await.expect("recover running work");
    assert_eq!(recovered.requeued, 1);
    assert_eq!(recovered.delivery_unknown, 0);

    let replay = execute(&store).await;
    assert_eq!(replay.request_id, work.request_id);
    store
        .mark_running(replay.update_id)
        .await
        .expect("restart work");
    store
        .set_result(replay.update_id, "durable answer".to_owned())
        .await
        .expect("store result");
    let PendingAction::Deliver(delivery) = store
        .next_action()
        .await
        .expect("load delivery")
        .expect("ready delivery")
    else {
        panic!("result did not become delivery");
    };
    store
        .mark_delivering(delivery.update_id)
        .await
        .expect("begin delivery");
    let recovered = store.recover().await.expect("recover delivery");
    assert_eq!(recovered.requeued, 0);
    assert_eq!(recovered.delivery_unknown, 1);
    assert!(store.next_action().await.expect("load queue").is_none());
}

#[tokio::test]
async fn identity_binding_rejects_a_different_bot_owner_or_workspace() {
    let directory = tempdir().expect("temporary data root");
    let workspace = directory.path().join("workspace");
    let other = directory.path().join("other");
    std::fs::create_dir(&workspace).expect("create workspace");
    std::fs::create_dir(&other).expect("create other workspace");
    let store = store(directory.path(), &workspace).await;
    assert!(store.bind_identity(10, OWNER, &workspace).await.is_err());
    assert!(store.bind_identity(9, OWNER + 1, &workspace).await.is_err());
    assert!(store.bind_identity(9, OWNER, &other).await.is_err());
}

async fn execute(store: &SurfaceStore) -> super::WorkItem {
    let PendingAction::Execute(work) = store
        .next_action()
        .await
        .expect("load action")
        .expect("queued action")
    else {
        panic!("expected execution");
    };
    work
}

async fn settle(store: &SurfaceStore, update_id: i64) {
    store.mark_running(update_id).await.expect("start work");
    store
        .set_result(update_id, "done".to_owned())
        .await
        .expect("store result");
    let PendingAction::Deliver(delivery) = store
        .next_action()
        .await
        .expect("load delivery")
        .expect("ready delivery")
    else {
        panic!("expected delivery");
    };
    store
        .mark_delivering(update_id)
        .await
        .expect("begin delivery");
    store
        .mark_chunk_delivered(update_id, delivery.cursor, 500 + update_id, true)
        .await
        .expect("complete delivery");
}
