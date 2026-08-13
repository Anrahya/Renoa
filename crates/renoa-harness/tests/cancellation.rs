use std::{num::NonZeroU32, sync::Arc, sync::atomic::Ordering, time::Duration};

use renoa_agent::{ContentBlock, Message};
use renoa_harness::{
    CancellationId, Harness, OperationOutcome, OperationRequest, OperationStatus, RequestId,
    RunNext, RuntimeProfile, SessionId, ToolBinding, ToolRecovery,
};
use tempfile::tempdir;

#[path = "cancellation/identity.rs"]
mod identity;
#[path = "cancellation/support.rs"]
mod support;

use support::{
    CooperativeTool, NeverCalledModel, NeverCalledTool, OneResponseModel, PendingModel, bash_call,
    tool_response,
};

const CANCELLED_BY_CALLER: &str = "operation was cancelled by the caller";

#[tokio::test]
async fn an_active_model_operation_can_be_cancelled_durably() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("keep working")]),
        )
        .await
        .expect("admit operation");
    let model = Arc::new(PendingModel::default());
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let driver = Arc::clone(&harness);
    let run = tokio::spawn(async move { driver.run_next(session_id, &profile).await });
    model.started.notified().await;
    let cancellation_id = CancellationId::new();

    harness
        .request_standalone_cancellation(session_id, admission.operation_id, cancellation_id)
        .await
        .expect("persist cancellation");

    assert_eq!(
        run.await.expect("join driver").expect("cancel operation"),
        RunNext::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Cancelled {
                message: CANCELLED_BY_CALLER.to_owned(),
            },
        }
    );
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, cancellation_id)
        .await
        .expect("retry cancellation after its reply was lost");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Cancelled);
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(
        snapshot.outputs[0].outcome,
        OperationOutcome::Cancelled {
            message: CANCELLED_BY_CALLER.to_owned(),
        }
    );

    drop(harness);
    let harness = Harness::open(&database).expect("reopen harness");
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect reopened session")
            .operations[0]
            .status,
        OperationStatus::Cancelled
    );
}

#[tokio::test]
async fn cancellation_recorded_without_a_live_driver_prevents_model_replay() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("keep working")]),
        )
        .await
        .expect("admit operation");
    let model = Arc::new(PendingModel::default());
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let driver = Arc::clone(&harness);
    let run = tokio::spawn(async move { driver.run_next(session_id, &profile).await });
    model.started.notified().await;
    run.abort();
    assert!(run.await.expect_err("aborted driver").is_cancelled());

    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("persist cancellation without a live driver");
    drop(harness);

    let harness = Harness::open(&database).expect("reopen harness");
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "changed instructions must not matter",
        NonZeroU32::new(9).expect("non-zero attempt limit"),
    );
    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("settle durable cancellation"),
        RunNext::Finished {
            operation_id,
            outcome: OperationOutcome::Cancelled { ref message },
        } if operation_id == admission.operation_id && message == CANCELLED_BY_CALLER
    ));
}

#[tokio::test]
async fn cancellation_waits_for_the_running_tool_and_skips_the_rest_of_its_batch() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("run it")]),
        )
        .await
        .expect("admit operation");
    let first_call = bash_call("call-1", "long-running-command");
    let second_call = bash_call("call-2", "must-not-run");
    let model = Arc::new(OneResponseModel(tool_response([
        first_call.clone(),
        second_call.clone(),
    ])));
    let tool = Arc::new(CooperativeTool::new());
    let profile = RuntimeProfile::new(
        "coding-v1",
        model,
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(tool.clone(), ToolRecovery::NeverReplay)],
        NonZeroU32::new(2).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let driver = Arc::clone(&harness);
    let mut run = tokio::spawn(async move { driver.run_next(session_id, &profile).await });
    tool.started.notified().await;

    let cancellation_id = CancellationId::new();
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, cancellation_id)
        .await
        .expect("persist cancellation");
    tool.cancellation_seen.notified().await;
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, cancellation_id)
        .await
        .expect("retry live cancellation after its reply was lost");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut run)
            .await
            .is_err(),
        "the operation must not settle while the tool still owns live work"
    );
    tool.allow_stop.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .expect("cooperative tool cancellation must settle")
        .expect("join driver")
        .expect("cancel operation");

    assert!(tool.stopped.load(Ordering::SeqCst));
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        result,
        RunNext::Finished {
            operation_id,
            outcome: OperationOutcome::Cancelled { ref message },
        } if operation_id == admission.operation_id && message == CANCELLED_BY_CALLER
    ));
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Cancelled);
    let results = snapshot
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { result } => Some(result),
            Message::User { .. } | Message::Assistant { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].call_id, first_call.id);
    assert_eq!(
        results[0].content,
        vec![ContentBlock::text("Tool execution was cancelled.")]
    );
    assert_eq!(results[1].call_id, second_call.id);
    assert_eq!(
        results[1].content,
        vec![ContentBlock::text(
            "Tool call was not executed because the operation was cancelled."
        )]
    );
    assert!(results.iter().all(|result| result.is_error));
}

#[tokio::test]
async fn cancellation_after_driver_loss_never_replays_a_pending_tool() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("run it")]),
        )
        .await
        .expect("admit operation");
    let call = bash_call("call-1", "long-running-command");
    let tool = Arc::new(CooperativeTool::new());
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(OneResponseModel(tool_response([call]))),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(tool.clone(), ToolRecovery::SafeToReplay)],
        NonZeroU32::new(2).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let driver = Arc::clone(&harness);
    let run = tokio::spawn(async move { driver.run_next(session_id, &profile).await });
    tool.started.notified().await;
    run.abort();
    assert!(run.await.expect_err("aborted driver").is_cancelled());

    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("persist cancellation without a live driver");
    drop(harness);

    let harness = Harness::open(&database).expect("reopen harness");
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(
            Arc::new(NeverCalledTool::new()),
            ToolRecovery::SafeToReplay,
        )],
        NonZeroU32::new(2).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    assert_eq!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("recover cancelled pending tool"),
        RunNext::Blocked {
            operation_id: admission.operation_id,
        }
    );
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect blocked session")
            .operations[0]
            .status,
        OperationStatus::OutcomeUnknown
    );
}
