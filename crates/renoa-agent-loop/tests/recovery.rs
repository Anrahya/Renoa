use std::sync::{Arc, Mutex};

use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{
    DriveResult, EffectOutcome, EffectRecovery, EffectStatus, EventCursor, Kernel, KernelError,
    OperationOutcome, OperationStatus,
};
use tempfile::tempdir;

#[path = "recovery/fakes.rs"]
mod fakes;
use fakes::*;

#[tokio::test]
async fn interrupted_model_replays_the_exact_persisted_request() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    submit_text(&kernel, session_id, "Remember this exact request.");
    let runtime = test_runtime(Arc::new(PanickingModel), Vec::new());
    let driver = Arc::clone(&kernel);
    let task = tokio::spawn(async move { driver.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("model panic").is_panic());

    let interrupted = kernel
        .inspect(session_id)
        .expect("inspect interrupted model");
    let original_effect = interrupted.operations[0].effects[0].clone();
    assert_eq!(original_effect.status, EffectStatus::DispatchStarted);
    assert_eq!(original_effect.dispatch_count, 1);
    drop(kernel);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let resumed = test_runtime(
        Arc::new(RecordingModel::new(
            [text_response("Recovered.")],
            Arc::clone(&requests),
        )),
        Vec::new(),
    );
    assert!(matches!(
        kernel
            .drive(session_id, &resumed)
            .await
            .expect("recover model effect"),
        DriveResult::Finished { .. }
    ));

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].messages,
        vec![renoa_agent::Message::user_text(
            "Remember this exact request."
        )]
    );
    drop(requests);
    let recovered = kernel.inspect(session_id).expect("inspect recovered model");
    let replayed_effect = &recovered.operations[0].effects[0];
    assert_eq!(replayed_effect.effect_id, original_effect.effect_id);
    assert_eq!(replayed_effect.request, original_effect.request);
    assert_eq!(replayed_effect.dispatch_count, 2);
    assert_eq!(replayed_effect.status, EffectStatus::Settled);
}

#[tokio::test]
async fn interrupted_never_replay_tool_becomes_unknown_without_invocation() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    submit_text(&kernel, session_id, "Perform the unsafe action.");
    let runtime = test_runtime(
        Arc::new(RecordingModel::new(
            [tool_response(tool_call("unsafe-1", "unsafe_action"))],
            Arc::new(Mutex::new(Vec::new())),
        )),
        vec![AgentToolBinding::new(
            "unsafe-action-v1",
            Arc::new(PanickingTool),
            EffectRecovery::NeverReplay,
        )],
    );
    let driver = Arc::clone(&kernel);
    let task = tokio::spawn(async move { driver.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("tool panic").is_panic());
    let interrupted = kernel
        .inspect(session_id)
        .expect("inspect interrupted tool");
    assert_eq!(interrupted.operations[0].effects.len(), 2);
    assert_eq!(
        interrupted.operations[0].effects[1].status,
        EffectStatus::DispatchStarted
    );
    drop(kernel);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let kernel = Kernel::open(&database).expect("reopen kernel");
    let resumed = test_runtime(
        Arc::new(NeverCalledModel),
        vec![AgentToolBinding::new(
            "unsafe-action-v1",
            Arc::new(RecordingTool {
                calls: Arc::clone(&calls),
            }),
            EffectRecovery::NeverReplay,
        )],
    );
    assert!(matches!(
        kernel
            .drive(session_id, &resumed)
            .await
            .expect("recover unsafe tool"),
        DriveResult::Blocked { .. }
    ));
    assert!(calls.lock().expect("tool calls lock").is_empty());
    let blocked = kernel.inspect(session_id).expect("inspect blocked tool");
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        blocked.operations[0].effects[1].status,
        EffectStatus::OutcomeUnknown
    );
}

#[tokio::test]
async fn uncertain_model_failure_blocks_the_operation_without_settling_it() {
    let (result, blocked) = drive_one_model(Arc::new(UncertainModel)).await;
    assert!(matches!(result, DriveResult::Blocked { .. }));
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        blocked.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(blocked.operations[0].effects[0].outcome, None);
}

#[tokio::test]
async fn incomplete_model_stream_blocks_the_operation_without_settling_it() {
    let (result, blocked) = drive_one_model(Arc::new(IncompleteModel)).await;
    assert!(matches!(result, DriveResult::Blocked { .. }));
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        blocked.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(blocked.operations[0].effects[0].outcome, None);
}

#[tokio::test]
async fn known_pre_inference_rejection_remains_a_definite_failure() {
    let (result, failed) = drive_one_model(Arc::new(RejectedModel)).await;
    assert!(matches!(
        result,
        DriveResult::Finished {
            outcome: OperationOutcome::Failed { .. },
            ..
        }
    ));
    assert_eq!(failed.operations[0].status, OperationStatus::Failed);
    assert_eq!(
        failed.operations[0].effects[0].status,
        EffectStatus::Settled
    );
    assert!(failed.operations[0].effects[0].outcome.is_some());
}

#[tokio::test]
async fn known_network_failure_before_inference_is_a_definite_failure() {
    let (result, failed) = drive_one_model(Arc::new(NetworkRejectedModel)).await;
    assert!(matches!(
        result,
        DriveResult::Finished {
            outcome: OperationOutcome::Failed { ref reason },
            ..
        } if reason.contains("connection refused before dispatch (ECONNREFUSED)")
    ));
    assert_eq!(failed.operations[0].status, OperationStatus::Failed);
    assert_eq!(
        failed.operations[0].effects[0].status,
        EffectStatus::Settled
    );
}

#[tokio::test]
async fn post_dispatch_reset_blocks_without_a_definite_failure() {
    let (result, blocked) = drive_one_model(Arc::new(PostDispatchResetModel)).await;
    assert!(matches!(result, DriveResult::Blocked { .. }));
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        blocked.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(blocked.operations[0].effects[0].outcome, None);
}

#[tokio::test]
async fn a_completed_model_result_survives_kernel_cancellation() {
    let directory = tempdir().expect("temporary directory");
    let invoked = Arc::new(tokio::sync::Notify::new());
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let session_id = create_session(&kernel);
    submit_text(&kernel, session_id, "Return a definite result.");
    let runtime = test_runtime(
        Arc::new(CompletesAfterCancelModel {
            invoked: Arc::clone(&invoked),
        }),
        Vec::new(),
    );
    let driver = Arc::clone(&kernel);
    let drive = tokio::spawn(async move { driver.drive(session_id, &runtime).await });
    invoked.notified().await;
    let operation_id = kernel
        .inspect(session_id)
        .expect("inspect in-flight operation")
        .operations[0]
        .operation_id;
    kernel
        .request_cancellation(
            session_id,
            operation_id,
            renoa_kernel::CancellationId::new(),
        )
        .expect("request cancellation");
    let result = drive
        .await
        .expect("join drive")
        .expect("drive completed result");
    assert!(matches!(result, DriveResult::Finished { .. }), "{result:?}");
    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect completed result");
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::Settled
    );
    let output = snapshot.operations[0].effects[0]
        .outcome
        .as_ref()
        .expect("definite model result must settle");
    let text = match output {
        EffectOutcome::Success(value) => value.to_string(),
        EffectOutcome::Failure { message } => message.clone(),
        _ => format!("{output:?}"),
    };
    assert!(
        text.contains("definite"),
        "cancelled drain must keep the completed model output: {text}"
    );
}

#[tokio::test]
async fn a_frozen_pi_model_revision_cannot_execute_on_the_native_adapter_identity() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    submit_text(&kernel, session_id, "Freeze this unfinished operation.");
    let runtime = test_runtime_with_revision(
        "pi/xai/grok-4.6/old-binding/reasoning-high",
        Arc::new(PanickingModel),
        Vec::new(),
    );
    let driver = Arc::clone(&kernel);
    let task = tokio::spawn(async move { driver.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("model panic").is_panic());
    drop(kernel);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let resumed = test_runtime_with_revision(
        "renoa-model-provider-node/v1/xai/grok-4.6/old-binding/reasoning-high",
        Arc::new(NeverCalledModel),
        Vec::new(),
    );
    assert!(matches!(
        kernel.drive(session_id, &resumed).await,
        Err(KernelError::RuntimeMismatch)
    ));
}

#[tokio::test]
async fn uncertain_live_tool_outcome_blocks_without_recording_a_false_result() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    submit_text(&kernel, session_id, "Perform the external action once.");
    let tool_calls = Arc::new(Mutex::new(Vec::new()));
    let model_requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = test_runtime(
        Arc::new(RecordingModel::new(
            [tool_response(tool_call("unsafe-live-1", "unsafe_action"))],
            Arc::clone(&model_requests),
        )),
        vec![AgentToolBinding::new(
            "unsafe-action-v1",
            Arc::new(UncertainTool {
                calls: Arc::clone(&tool_calls),
            }),
            EffectRecovery::NeverReplay,
        )],
    );

    assert!(matches!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive uncertain tool"),
        DriveResult::Blocked { .. }
    ));
    assert_eq!(tool_calls.lock().expect("tool calls lock").len(), 1);
    assert_eq!(model_requests.lock().expect("model requests lock").len(), 1);

    let blocked = kernel.inspect(session_id).expect("inspect blocked tool");
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(blocked.operations[0].effects.len(), 2);
    assert_eq!(
        blocked.operations[0].effects[1].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(blocked.operations[0].effects[1].outcome, None);

    let history = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read blocked history");
    assert_eq!(history.events.len(), 2);
    let messages = history
        .events
        .into_iter()
        .map(|event| {
            serde_json::from_value::<renoa_agent::Message>(event.payload)
                .expect("decode message event")
        })
        .collect::<Vec<_>>();
    assert!(matches!(messages[0], renoa_agent::Message::User { .. }));
    assert!(matches!(
        messages[1],
        renoa_agent::Message::Assistant { .. }
    ));
}
