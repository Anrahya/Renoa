use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::{AssistantContent, AssistantMetadata, ContentBlock, Message, StopReason};
use tempfile::tempdir;

use super::support::{
    FailThenCompleteModel, FailingModel, IncompleteStreamModel, NeverCalledModel, RecordingModel,
    UnexpectedToolCallModel, create_session, response_with_usage,
};
use crate::{
    Harness, HarnessError, OperationOutcome, OperationRequest, OperationStatus, RequestId, RunNext,
    RuntimeProfile, SessionId,
    drive::{PendingRecovery, Settlement},
    inspect_model_attempts,
};

#[tokio::test]
async fn retrying_session_creation_with_the_same_identity_is_safe() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let session_id = SessionId::new();
    let harness = Harness::open(&database).expect("open harness");

    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .create_standalone_session(session_id)
        .await
        .expect("retry session creation");
    drop(harness);

    let harness = Harness::open(&database).expect("reopen harness");
    harness
        .create_standalone_session(session_id)
        .await
        .expect("retry session creation after lost reply");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert!(snapshot.messages.is_empty());
    assert!(snapshot.operations.is_empty());
    assert!(snapshot.outputs.is_empty());
}

#[tokio::test]
async fn a_stale_attempt_cannot_settle_current_work() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let lease = harness.begin_run(session_id).expect("own session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("continue")]),
        )
        .await
        .expect("admit operation");
    let activation_profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let active = harness
        .store
        .activate(&lease, session_id, activation_profile.frozen())
        .await
        .expect("activate")
        .expect("active operation");
    let crate::drive::ModelStart::Invoke(first) = harness
        .store
        .begin_model_attempt(&lease, active.operation_id, None)
        .await
        .expect("first intent")
    else {
        panic!("uncancelled operation must create a model intent");
    };
    let PendingRecovery::Retry(second) = harness
        .store
        .recover_model_attempt(&lease, active.operation_id)
        .await
        .expect("recover first attempt")
    else {
        panic!("retry must remain available");
    };

    assert!(matches!(
        harness
            .store
            .settle_model(&lease, first, response_with_usage())
            .await
            .expect("stale settlement is a no-op"),
        Settlement::Stale
    ));
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.messages, vec![Message::user_text("continue")]);
    assert!(snapshot.outputs.is_empty());
    assert_eq!(snapshot.operations[0].status, OperationStatus::Running);

    assert!(matches!(
        harness
            .store
            .settle_model(&lease, second, response_with_usage())
            .await
            .expect("settle current attempt"),
        Settlement::Applied(_)
    ));
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.outputs.len(), 1);
}

#[tokio::test]
async fn an_unexpected_tool_call_fails_closed() {
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
    let model = Arc::new(UnexpectedToolCallModel::default());
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    );

    let result = harness
        .run_next(session_id, &profile)
        .await
        .expect("reject invalid response");
    assert!(matches!(
        result,
        RunNext::Finished {
            outcome: OperationOutcome::Failed { .. },
            ..
        }
    ));
    assert!(
        model
            .requests()
            .iter()
            .all(|request| request.tools.is_empty())
    );
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert_eq!(snapshot.messages, vec![Message::user_text("continue")]);
    assert_eq!(
        inspect_model_attempts(&harness, session_id)[0].status,
        "completed"
    );
    assert_eq!(snapshot.outputs.len(), 1);
    assert!(matches!(
        snapshot.outputs[0].outcome,
        OperationOutcome::Failed { .. }
    ));

    drop(harness);
    let harness = Harness::open(&database).expect("reopen harness");
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
            .expect("run terminal session"),
        RunNext::Idle
    );
}

#[tokio::test]
async fn an_uncertain_model_failure_uses_the_saved_retry_budget() {
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
    let model = Arc::new(FailThenCompleteModel::default());
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );

    let result = harness
        .run_next(session_id, &profile)
        .await
        .expect("retry uncertain failure");
    assert!(matches!(
        result,
        RunNext::Finished {
            outcome: OperationOutcome::Completed { .. },
            ..
        }
    ));
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(attempts[0].status, "outcome_unknown");
    assert_eq!(
        attempts[0].error.as_deref(),
        Some("model invocation failed: temporary failure")
    );
    assert_eq!(attempts[1].status, "completed");
    assert!(attempts.iter().all(|attempt| !attempt.has_request));
}

#[tokio::test]
async fn an_exhausted_model_error_is_durable_after_reopen() {
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
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(FailingModel("provider down")),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    );

    let result = harness
        .run_next(session_id, &profile)
        .await
        .expect("record failed operation");
    assert!(matches!(
        result,
        RunNext::Finished {
            outcome: OperationOutcome::Failed { ref message },
            ..
        } if message == "model invocation failed: provider down"
    ));
    drop(harness);

    let harness = Harness::open(&database).expect("reopen harness");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert_eq!(
        snapshot.outputs[0].outcome,
        OperationOutcome::Failed {
            message: "model invocation failed: provider down".to_owned(),
        }
    );
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(attempts[0].status, "outcome_unknown");
    assert_eq!(attempts[0].usage, None);
    assert!(!attempts[0].has_request);
    assert_eq!(
        attempts[0].error.as_deref(),
        Some("model invocation failed: provider down")
    );
}

#[tokio::test]
async fn an_incomplete_stream_is_uncertain_and_has_no_usage() {
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
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(IncompleteStreamModel),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    );

    harness
        .run_next(session_id, &profile)
        .await
        .expect("record incomplete stream");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(
        snapshot.outputs[0].outcome,
        OperationOutcome::Failed {
            message: "model stream ended without a completed response".to_owned(),
        }
    );
    let attempts = inspect_model_attempts(&harness, session_id);
    assert_eq!(attempts[0].status, "outcome_unknown");
    assert_eq!(attempts[0].usage, None);
    assert!(!attempts[0].has_request);
    assert_eq!(
        attempts[0].error.as_deref(),
        Some("model stream ended without a completed response")
    );
}

#[tokio::test]
async fn queued_input_enters_context_only_when_its_operation_activates() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("first")]),
        )
        .await
        .expect("admit first operation");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("second")]),
        )
        .await
        .expect("admit second operation");
    let model = Arc::new(RecordingModel::default());
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    );

    harness
        .run_next(session_id, &profile)
        .await
        .expect("run first operation");
    let first_snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(first_snapshot.messages.len(), 2);
    assert_eq!(first_snapshot.operations[1].status, OperationStatus::Queued);

    harness
        .run_next(session_id, &profile)
        .await
        .expect("run second operation");
    let requests = model.requests();
    assert_eq!(requests[0].messages, vec![Message::user_text("first")]);
    assert_eq!(
        requests[1].messages,
        vec![
            Message::user_text("first"),
            Message::Assistant {
                content: vec![AssistantContent::text("done")],
                stop_reason: StopReason::Stop,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
            Message::user_text("second"),
        ]
    );
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(
        snapshot
            .operations
            .iter()
            .map(|operation| operation.status)
            .collect::<Vec<_>>(),
        vec![OperationStatus::Completed, OperationStatus::Completed]
    );
    assert_eq!(
        snapshot
            .outputs
            .iter()
            .map(|output| output.sequence)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[test]
fn a_newer_schema_version_fails_closed() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let connection = rusqlite::Connection::open(&database).expect("open SQLite database");
    connection
        .pragma_update(None, "user_version", 5)
        .expect("set newer schema version");
    drop(connection);

    assert_eq!(
        Harness::open(&database)
            .err()
            .expect("newer schema must be rejected"),
        HarnessError::UnsupportedSchema {
            found: 5,
            supported: 4,
        }
    );
}
