use std::{num::NonZeroU32, sync::Arc, time::Duration};

use renoa_agent::{ContentBlock, Message, ModelRequest, TokenUsage};
use tempfile::tempdir;

use super::support::{
    FixedResponseModel, NeverCalledModel, PendingRecordingModel, RecordingModel, create_session,
    response_with_usage,
};
use crate::{
    CrashPoint, Harness, HarnessError, OperationRequest, OperationStatus, RequestId, RunNext,
    RuntimeProfile, inspect_model_attempts,
};

#[tokio::test]
async fn model_intent_is_durable_before_dispatch() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("continue")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ModelIntentCommitted);
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );

    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Running);
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].status, "pending");
    assert!(
        attempts[0].has_request,
        "pending attempt must retain its request"
    );

    let model = Arc::new(RecordingModel::default());
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Changed instructions.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    harness
        .run_next(session_id, &profile)
        .await
        .expect("recover pending attempt");
    assert_eq!(
        model.requests(),
        vec![ModelRequest {
            system_prompt: "Be precise.".to_owned(),
            messages: vec![Message::user_text("continue")],
            tools: Vec::new(),
        }]
    );
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| attempt.status.as_str())
            .collect::<Vec<_>>(),
        vec!["outcome_unknown", "completed"]
    );
    assert!(
        attempts.iter().all(|attempt| !attempt.has_request),
        "retired attempts must release serialized requests"
    );
}

#[tokio::test]
async fn a_dispatched_request_reopens_as_uncertain() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("continue")]),
        )
        .await
        .expect("admit operation");
    let model = Arc::new(PendingRecordingModel::default());
    let called = model.called.notified();
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let harness = Arc::new(harness);
    let runner = Arc::clone(&harness);
    let task = tokio::spawn(async move { runner.run_next(session_id, &profile).await });
    tokio::time::timeout(Duration::from_secs(2), called)
        .await
        .expect("model dispatch");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("queued later")]),
        )
        .await
        .expect("admit while model runs");
    let competing_profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    assert_eq!(
        harness
            .run_next(session_id, &competing_profile)
            .await
            .expect_err("second driver must be rejected"),
        HarnessError::Busy(session_id)
    );
    let running_snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(
        running_snapshot.messages,
        vec![Message::user_text("continue")]
    );
    assert_eq!(
        running_snapshot.operations[1].status,
        OperationStatus::Queued
    );
    task.abort();
    assert!(task.await.expect_err("aborted driver").is_cancelled());
    let first_request = model.requests().into_iter().next().expect("first request");
    drop(harness);

    let harness = Harness::open(&database).expect("reopen harness");
    let model = Arc::new(RecordingModel::default());
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    harness
        .run_next(session_id, &profile)
        .await
        .expect("recover pending attempt");
    assert_eq!(model.requests(), vec![first_request]);
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(attempts[0].status, "outcome_unknown");
    assert_eq!(attempts[1].status, "completed");
    assert!(attempts.iter().all(|attempt| !attempt.has_request));
}

#[tokio::test]
async fn a_completed_but_unsettled_response_is_not_partially_visible() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("continue")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ModelCompletedBeforeSettlement);
    let response = response_with_usage();
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(FixedResponseModel(response.clone())),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );

    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Running);
    assert_eq!(snapshot.messages, vec![Message::user_text("continue")]);
    assert!(snapshot.outputs.is_empty());
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(attempts[0].usage, None);
    assert!(attempts[0].has_request);

    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(FixedResponseModel(response)),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    harness
        .run_next(session_id, &profile)
        .await
        .expect("recover pending attempt");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.outputs.len(), 1);
    assert_eq!(snapshot.outputs[0].sequence, 0);
    let attempts = inspect_model_attempts(&harness, session_id);
    assert!(attempts.iter().all(|attempt| !attempt.has_request));
    assert_eq!(
        attempts[1].usage,
        Some(TokenUsage {
            input: 11,
            output: 3,
            cache_read: 2,
            cache_write: 1,
        })
    );
}

#[tokio::test]
async fn a_settled_response_is_never_sampled_again() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("continue")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::SettlementCommitted);
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(FixedResponseModel(response_with_usage())),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    );

    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Completed);
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.outputs.len(), 1);
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(attempts[0].status, "completed");
    assert!(!attempts[0].has_request);

    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    );
    assert_eq!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("run settled session"),
        RunNext::Idle
    );
}
