use std::{
    num::NonZeroU32,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, Model, ModelError,
    ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall,
    ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, AgentToolBinding, ContextBinding, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, CancellationId, Command, CommandId, DriveResult, EffectRecovery, EventCursor, Kernel,
    OperationOutcome, OperationStatus, SessionId,
};
use tempfile::tempdir;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancelling_model_work_does_not_invent_an_assistant_message() {
    let directory = tempdir().expect("temporary directory");
    let invoked = Arc::new(Notify::new());
    let runtime = Arc::new(
        build_runtime(
            config(),
            ContextBinding::full_history(),
            ModelBinding::new(
                "cancellable-model-v1",
                Arc::new(CancellableModel {
                    invoked: Arc::clone(&invoked),
                }),
                EffectRecovery::SafeToReplay,
            ),
            Vec::new(),
        )
        .expect("build runtime"),
    );
    let (kernel, session_id, operation_id) = kernel_with_command(&directory, "Wait for model.");
    let kernel = Arc::new(kernel);
    let runner = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let drive =
        tokio::spawn(async move { runner.drive(session_id, driven_runtime.as_ref()).await });
    invoked.notified().await;

    kernel
        .request_cancellation(session_id, operation_id, CancellationId::new())
        .expect("request cancellation");
    assert_eq!(
        drive
            .await
            .expect("join drive")
            .expect("settle cancellation"),
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Cancelled,
        }
    );
    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read events")
        .events;
    assert_eq!(events.len(), 1);
    assert!(matches!(
        decode_message(&events[0].payload),
        Message::User { .. }
    ));
    assert_eq!(
        kernel
            .inspect(session_id)
            .expect("inspect operation")
            .operations[0]
            .status,
        OperationStatus::Cancelled
    );
}

#[tokio::test]
async fn uncertain_cancelled_tool_is_honest_and_later_calls_are_not_run() {
    let directory = tempdir().expect("temporary directory");
    let tool_invoked = Arc::new(Notify::new());
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(OneResponseModel(Mutex::new(Some(ModelResponse {
        content: vec![
            AssistantContent::tool_call(tool_call("first")),
            AssistantContent::tool_call(tool_call("second")),
        ],
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }))));
    let tool = Arc::new(UnknownOnCancellationTool {
        spec: ToolSpec {
            name: "uncertain".to_owned(),
            description: "Waits for cancellation.".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        invoked: Arc::clone(&tool_invoked),
        calls: Arc::clone(&tool_calls),
    });
    let runtime = Arc::new(
        build_runtime(
            config(),
            ContextBinding::full_history(),
            ModelBinding::new("one-response-v1", model, EffectRecovery::SafeToReplay),
            vec![AgentToolBinding::new(
                "uncertain-tool-v1",
                tool,
                EffectRecovery::NeverReplay,
            )],
        )
        .expect("build runtime"),
    );
    let (kernel, session_id, operation_id) = kernel_with_command(&directory, "Use both tools.");
    let kernel = Arc::new(kernel);
    let runner = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let drive =
        tokio::spawn(async move { runner.drive(session_id, driven_runtime.as_ref()).await });
    tool_invoked.notified().await;

    kernel
        .request_cancellation(session_id, operation_id, CancellationId::new())
        .expect("request cancellation");
    assert!(matches!(
        drive
            .await
            .expect("join drive")
            .expect("settle cancellation"),
        DriveResult::Finished {
            outcome: OperationOutcome::Cancelled,
            ..
        }
    ));
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read events")
        .events;
    assert_eq!(events.len(), 4);
    let Message::Tool { result: current } = decode_message(&events[2].payload) else {
        panic!("current tool call has no repair result")
    };
    let Message::Tool { result: later } = decode_message(&events[3].payload) else {
        panic!("later tool call has no repair result")
    };
    assert_eq!(current.call_id, "first");
    assert_eq!(later.call_id, "second");
    assert!(current.is_error && later.is_error);
    assert!(tool_text(&current.content).contains("may have finished"));
    assert!(tool_text(&later.content).contains("was not run"));
}

fn config() -> AgentLoopConfig {
    AgentLoopConfig::new(
        "Be precise.",
        NonZeroU32::new(3).expect("non-zero model limit"),
        NonZeroU32::new(3).expect("non-zero tool limit"),
    )
}

fn kernel_with_command(
    directory: &tempfile::TempDir,
    text: &str,
) -> (Kernel, SessionId, renoa_kernel::OperationId) {
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let admission = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text(text)).expect("serialize command"),
            ),
        )
        .expect("submit command");
    (kernel, session_id, admission.operation_id)
}

struct CancellableModel {
    invoked: Arc<Notify>,
}

impl Model for CancellableModel {
    fn stream(
        &self,
        _request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let invoked = Arc::clone(&self.invoked);
        stream::once(async move {
            invoked.notify_one();
            cancellation.cancelled().await;
            Err(ModelError::new("cancelled model stream"))
        })
        .boxed()
    }
}

struct OneResponseModel(Mutex<Option<ModelResponse>>);

impl Model for OneResponseModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let response = self
            .0
            .lock()
            .expect("model response lock")
            .take()
            .ok_or_else(|| ModelError::new("model response already used"));
        stream::once(async move { response.map(|response| ModelEvent::Completed { response }) })
            .boxed()
    }
}

struct UnknownOnCancellationTool {
    spec: ToolSpec,
    invoked: Arc<Notify>,
    calls: Arc<AtomicUsize>,
}

impl Tool for UnknownOnCancellationTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.invoked.notify_one();
            cancellation.cancelled().await;
            Err(ToolError::outcome_unknown(
                "the remote action may have completed",
            ))
        })
    }
}

fn tool_call(id: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: "uncertain".to_owned(),
        arguments: serde_json::json!({}),
        thought_signature: None,
        namespace: None,
    }
}

fn decode_message(value: &serde_json::Value) -> Message {
    serde_json::from_value(value.clone()).expect("decode message event")
}

fn tool_text(content: &[ContentBlock]) -> &str {
    let [ContentBlock::Text { text }] = content else {
        panic!("tool repair must contain one text block")
    };
    text
}
