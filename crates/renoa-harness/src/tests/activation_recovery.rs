use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::{ContentBlock, Message, ModelRequest};
use tempfile::tempdir;

use super::support::{CompletingModel, NeverCalledModel, RecordingModel, create_session};
use crate::{
    CrashPoint, Harness, HarnessError, OperationOutcome, OperationRequest, OperationStatus,
    RequestId, RunNext, RuntimeProfile, inspect_model_attempts,
};

#[tokio::test]
async fn activation_survives_without_duplicating_user_input() {
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
    harness.crash_at(CrashPoint::ActivationCommitted);
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
    assert_eq!(snapshot.messages, vec![Message::user_text("continue")]);
    assert_eq!(snapshot.operations[0].status, OperationStatus::Running);
    assert!(inspect_model_attempts(&harness, session_id).is_empty());

    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(CompletingModel),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    harness
        .run_next(session_id, &profile)
        .await
        .expect("complete recovered operation");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| matches!(message, Message::User { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn activation_freezes_the_system_prompt_not_just_its_revision() {
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
    harness.crash_at(CrashPoint::ActivationCommitted);
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Original instructions.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let model = Arc::new(RecordingModel::default());
    let changed_profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Changed instructions.",
        NonZeroU32::new(9).expect("non-zero attempt limit"),
    );
    harness
        .run_next(session_id, &changed_profile)
        .await
        .expect("recover activated operation");

    assert_eq!(
        model.requests(),
        vec![ModelRequest {
            system_prompt: "Original instructions.".to_owned(),
            messages: vec![Message::user_text("continue")],
            tools: Vec::new(),
        }]
    );
}

#[tokio::test]
async fn recovery_requires_the_frozen_profile_and_preserves_exhausted_limits() {
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
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    );
    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let wrong_profile = RuntimeProfile::new(
        "coding-v2",
        Arc::new(NeverCalledModel),
        "Different instructions.",
        NonZeroU32::new(3).expect("non-zero attempt limit"),
    );
    assert_eq!(
        harness
            .run_next(session_id, &wrong_profile)
            .await
            .expect_err("wrong profile must fail closed"),
        HarnessError::RuntimeProfileUnavailable {
            required: "coding-v1".to_owned(),
            provided: "coding-v2".to_owned(),
        }
    );
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(attempts[0].status, "pending");
    assert!(attempts[0].has_request);

    let correct_profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Changed instructions.",
        NonZeroU32::new(9).expect("non-zero attempt limit"),
    );
    let result = harness
        .run_next(session_id, &correct_profile)
        .await
        .expect("settle exhausted uncertain attempt");
    assert!(matches!(
        result,
        RunNext::Finished {
            outcome: OperationOutcome::Failed { .. },
            ..
        }
    ));
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert_eq!(snapshot.messages, vec![Message::user_text("continue")]);
    assert_eq!(snapshot.outputs.len(), 1);
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(attempts[0].status, "outcome_unknown");
    assert!(!attempts[0].has_request);
    assert_eq!(
        attempts[0].error.as_deref(),
        Some("process stopped before the model attempt settled")
    );
}
