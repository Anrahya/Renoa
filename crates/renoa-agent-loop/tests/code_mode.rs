use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{StreamExt as _, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, Model, ModelError,
    ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall,
    ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopBuildError, AgentLoopConfig, AgentToolBinding, CodeMcpCall,
    CodeModeBinding, CodeStep, CodeStepOutput, CodeStepRequest, ContextBinding, ModelBinding,
    build_runtime_with_code_mode,
};
use renoa_kernel::{
    AgentId, CancellationId, Command, CommandId, DriveResult, EffectAdapter, EffectCompletion,
    EffectFuture, EffectInvocation, EffectOutcome, EffectRecovery, EffectStatus, EventCursor,
    Kernel, OperationOutcome, Runtime, SessionId,
};
use serde_json::{Value, json};
use tokio::sync::{Barrier, Notify};
use tokio_util::sync::CancellationToken;

#[test]
fn code_mode_refuses_a_replayable_mcp_executor() {
    let model = Arc::new(ScriptedModel {
        responses: Mutex::new(VecDeque::new()),
        requests: Arc::new(Mutex::new(Vec::new())),
    });
    let result = build_runtime_with_code_mode(
        AgentLoopConfig::new(
            "Use Code Mode",
            NonZeroU32::new(1).expect("nonzero"),
            NonZeroU32::new(1).expect("nonzero"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("scripted", model, EffectRecovery::SafeToReplay),
        Vec::new(),
        CodeModeBinding::new(
            "fake-evaluator-v1",
            Arc::new(FakeEvaluator),
            AgentToolBinding::new(
                "mcp-v1",
                Arc::new(UnknownMcpTool),
                EffectRecovery::SafeToReplay,
            ),
        ),
    );
    assert!(matches!(
        result,
        Err(AgentLoopBuildError::ReplayableCodeModeExecutor)
    ));
}

#[tokio::test]
async fn parallel_mcp_calls_are_durable_and_only_one_code_result_enters_model_context() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent = AgentId::new();
    let session = SessionId::new();
    kernel.create_agent(agent).expect("create agent");
    kernel
        .create_session(session, agent)
        .expect("create session");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let dispatched = Arc::new(Mutex::new(Vec::new()));
    let runtime = scripted_runtime(&requests, &dispatched);
    let command = serde_json::to_value(AgentCommand::text("Do two MCP calls.")).expect("command");
    let admission = kernel
        .submit(session, Command::new(CommandId::new(), command))
        .expect("submit");
    let result = tokio::time::timeout(Duration::from_secs(3), kernel.drive(session, &runtime))
        .await
        .expect("MCP calls must run concurrently")
        .expect("drive");
    assert_eq!(
        result,
        DriveResult::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );

    let calls = dispatched.lock().expect("dispatch lock");
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].id, calls[1].id);
    assert!(
        calls
            .iter()
            .all(|call| call.name == "tool_execute" && call.id.starts_with("cm-"))
    );
    drop(calls);
    let snapshot = kernel.inspect(session).expect("inspect");
    let batches = &snapshot.operations[0].effect_batches;
    assert_eq!(batches.len(), 5);
    assert_eq!(batches[2].effects.len(), 2);
    assert!(
        batches[2]
            .effects
            .iter()
            .all(|effect| effect.recovery == EffectRecovery::NeverReplay)
    );
    let messages = kernel
        .events_after(session, EventCursor::START)
        .expect("events")
        .events
        .into_iter()
        .map(|event| serde_json::from_value::<Message>(event.payload).expect("message"))
        .collect::<Vec<_>>();
    assert_eq!(
        messages.len(),
        4,
        "nested MCP calls must not enter semantic history"
    );
    assert!(
        matches!(&messages[2], Message::Tool { result } if result.call_id == "outer" && result.name == "code_mode" && !result.is_error)
    );
    let requests = requests.lock().expect("model requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>(),
        vec!["code_mode"]
    );
    assert_eq!(requests[1].messages.len(), 3);
    assert!(
        matches!(&requests[1].messages[2], Message::Tool { result } if result.name == "code_mode")
    );
}

#[tokio::test]
async fn one_unknown_mcp_child_survives_restart_and_abandonment_balances_outer_calls() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let agent = AgentId::new();
    let session = SessionId::new();
    kernel.create_agent(agent).expect("create agent");
    kernel
        .create_session(session, agent)
        .expect("create session");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel {
        responses: Mutex::new(VecDeque::from([ModelResponse {
            content: vec![
                AssistantContent::tool_call(code_call("outer")),
                AssistantContent::tool_call(code_call("later")),
            ],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        }])),
        requests: Arc::clone(&requests),
    });
    let runtime = runtime(model, Arc::new(UnknownMcpTool), Arc::new(FakeEvaluator));
    let command = serde_json::to_value(AgentCommand::text("Call both.")).expect("command");
    let operation = kernel
        .submit(session, Command::new(CommandId::new(), command))
        .expect("submit")
        .operation_id;
    assert_eq!(
        kernel.drive(session, &runtime).await.expect("drive"),
        DriveResult::Blocked {
            operation_id: operation
        }
    );
    let before = kernel.inspect(session).expect("inspect");
    let children = &before.operations[0].effect_batches[2].effects;
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].status, EffectStatus::Settled);
    assert_eq!(children[1].status, EffectStatus::OutcomeUnknown);
    drop(kernel);
    let kernel = Kernel::open(&database).expect("reopen kernel after unknown MCP call");
    assert!(matches!(
        kernel
            .abandon_unknown_effect(session, operation, &runtime)
            .expect("abandon"),
        OperationOutcome::Failed { .. }
    ));
    let messages = kernel
        .events_after(session, EventCursor::START)
        .expect("events")
        .events
        .into_iter()
        .map(|event| serde_json::from_value::<Message>(event.payload).expect("message"))
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 4);
    assert!(
        matches!(&messages[2], Message::Tool { result } if result.call_id == "outer" && result.is_error)
    );
    assert!(
        matches!(&messages[3], Message::Tool { result } if result.call_id == "later" && result.is_error)
    );
    assert_eq!(requests.lock().expect("requests").len(), 1);
}

#[tokio::test]
async fn cancellation_during_nested_mcp_calls_balances_outer_calls() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let agent = AgentId::new();
    let session = SessionId::new();
    kernel.create_agent(agent).expect("create agent");
    kernel
        .create_session(session, agent)
        .expect("create session");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel {
        responses: Mutex::new(VecDeque::from([ModelResponse {
            content: vec![
                AssistantContent::tool_call(code_call("outer")),
                AssistantContent::tool_call(code_call("later")),
            ],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        }])),
        requests: Arc::clone(&requests),
    });
    let arrival = Arc::new(Barrier::new(3));
    let runtime = Arc::new(runtime(
        model,
        Arc::new(CancelledMcpTool {
            arrival: Arc::clone(&arrival),
        }),
        Arc::new(FakeEvaluator),
    ));
    let operation = kernel
        .submit(
            session,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text("Run and cancel.")).expect("command"),
            ),
        )
        .expect("submit")
        .operation_id;
    let runner = Arc::clone(&kernel);
    let running_runtime = Arc::clone(&runtime);
    let drive = tokio::spawn(async move { runner.drive(session, running_runtime.as_ref()).await });
    tokio::time::timeout(Duration::from_secs(3), arrival.wait())
        .await
        .expect("both MCP children must enter the adapter");
    kernel
        .request_cancellation(session, operation, CancellationId::new())
        .expect("request cancellation");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), drive)
            .await
            .expect("drive must finish")
            .expect("join drive")
            .expect("settle cancellation"),
        DriveResult::Finished {
            operation_id: operation,
            outcome: OperationOutcome::Cancelled,
        }
    );
    let messages = kernel
        .events_after(session, EventCursor::START)
        .expect("events")
        .events
        .into_iter()
        .map(|event| serde_json::from_value::<Message>(event.payload).expect("message"))
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 4);
    assert!(
        matches!(&messages[2], Message::Tool { result } if result.call_id == "outer" && result.is_error)
    );
    assert!(
        matches!(&messages[3], Message::Tool { result } if result.call_id == "later" && result.is_error)
    );
    assert_eq!(requests.lock().expect("requests").len(), 1);
}

#[tokio::test]
async fn cancellation_during_python_evaluation_balances_outer_calls() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let agent = AgentId::new();
    let session = SessionId::new();
    kernel.create_agent(agent).expect("create agent");
    kernel
        .create_session(session, agent)
        .expect("create session");
    let model = Arc::new(ScriptedModel {
        responses: Mutex::new(VecDeque::from([ModelResponse {
            content: vec![
                AssistantContent::tool_call(code_call("outer")),
                AssistantContent::tool_call(code_call("later")),
            ],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        }])),
        requests: Arc::new(Mutex::new(Vec::new())),
    });
    let invoked = Arc::new(Notify::new());
    let runtime = Arc::new(runtime(
        model,
        Arc::new(UnknownMcpTool),
        Arc::new(CancelledEvaluator {
            invoked: Arc::clone(&invoked),
        }),
    ));
    let operation = kernel
        .submit(
            session,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text("Cancel Python.")).expect("command"),
            ),
        )
        .expect("submit")
        .operation_id;
    let runner = Arc::clone(&kernel);
    let running_runtime = Arc::clone(&runtime);
    let drive = tokio::spawn(async move { runner.drive(session, running_runtime.as_ref()).await });
    tokio::time::timeout(Duration::from_secs(3), invoked.notified())
        .await
        .expect("Python evaluator must start");
    kernel
        .request_cancellation(session, operation, CancellationId::new())
        .expect("request cancellation");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), drive)
            .await
            .expect("drive must finish")
            .expect("join drive")
            .expect("settle cancellation"),
        DriveResult::Finished {
            operation_id: operation,
            outcome: OperationOutcome::Cancelled,
        }
    );
    let messages = kernel
        .events_after(session, EventCursor::START)
        .expect("events")
        .events
        .into_iter()
        .map(|event| serde_json::from_value::<Message>(event.payload).expect("message"))
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 4);
    assert!(
        matches!(&messages[2], Message::Tool { result } if result.call_id == "outer" && result.is_error)
    );
    assert!(
        matches!(&messages[3], Message::Tool { result } if result.call_id == "later" && result.is_error)
    );
    assert_eq!(
        kernel.inspect(session).expect("inspect").operations[0]
            .effect_batches
            .len(),
        2
    );
}

#[tokio::test]
async fn oversized_nested_results_are_not_sent_back_into_python() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent = AgentId::new();
    let session = SessionId::new();
    kernel.create_agent(agent).expect("create agent");
    kernel
        .create_session(session, agent)
        .expect("create session");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel {
        responses: Mutex::new(VecDeque::from([
            ModelResponse {
                content: vec![AssistantContent::tool_call(code_call("outer"))],
                stop_reason: StopReason::ToolUse,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
            ModelResponse {
                content: vec![AssistantContent::text("Done")],
                stop_reason: StopReason::Stop,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
        ])),
        requests: Arc::clone(&requests),
    });
    let runtime = runtime(model, Arc::new(OversizedMcpTool), Arc::new(FakeEvaluator));
    let operation = kernel
        .submit(
            session,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text("Read MCP data.")).expect("command"),
            ),
        )
        .expect("submit")
        .operation_id;
    assert_eq!(
        kernel.drive(session, &runtime).await.expect("drive"),
        DriveResult::Finished {
            operation_id: operation,
            outcome: OperationOutcome::Completed,
        }
    );
    let snapshot = kernel.inspect(session).expect("inspect");
    assert_eq!(snapshot.operations[0].effect_batches.len(), 4);
    let messages = kernel
        .events_after(session, EventCursor::START)
        .expect("events")
        .events
        .into_iter()
        .map(|event| serde_json::from_value::<Message>(event.payload).expect("message"))
        .collect::<Vec<_>>();
    assert!(
        matches!(&messages[2], Message::Tool { result } if result.name == "code_mode" && result.is_error && result.content.iter().any(|block| matches!(block, ContentBlock::Text { text } if text.contains("2 MiB"))))
    );
    assert_eq!(requests.lock().expect("requests").len(), 2);
}

fn scripted_runtime(
    requests: &Arc<Mutex<Vec<ModelRequest>>>,
    dispatched: &Arc<Mutex<Vec<ToolCall>>>,
) -> Runtime {
    let model = Arc::new(ScriptedModel {
        responses: Mutex::new(VecDeque::from([
            ModelResponse {
                content: vec![AssistantContent::tool_call(code_call("outer"))],
                stop_reason: StopReason::ToolUse,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
            ModelResponse {
                content: vec![AssistantContent::text("Done")],
                stop_reason: StopReason::Stop,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
        ])),
        requests: Arc::clone(requests),
    });
    let tool = Arc::new(ConcurrentMcpTool {
        spec: ToolSpec {
            name: "tool_execute".to_owned(),
            description: "Hidden MCP executor".to_owned(),
            input_schema: json!({"type": "object"}),
        },
        barrier: Arc::new(Barrier::new(2)),
        dispatched: Arc::clone(dispatched),
    });
    runtime(model, tool, Arc::new(FakeEvaluator))
}

fn runtime(
    model: Arc<dyn Model>,
    tool: Arc<dyn Tool>,
    evaluator: Arc<dyn EffectAdapter>,
) -> Runtime {
    build_runtime_with_code_mode(
        AgentLoopConfig::new(
            "Use Code Mode",
            NonZeroU32::new(4).expect("nonzero"),
            NonZeroU32::new(4).expect("nonzero"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("scripted-model", model, EffectRecovery::SafeToReplay),
        Vec::new(),
        CodeModeBinding::new(
            "fake-evaluator-v1",
            evaluator,
            AgentToolBinding::new("fake-mcp-v1", tool, EffectRecovery::NeverReplay),
        ),
    )
    .expect("runtime")
}

fn code_call(id: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: "code_mode".to_owned(),
        arguments: json!({"source": "await mcp('first', {})"}),
        thought_signature: None,
        namespace: None,
    }
}

struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl Model for ScriptedModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        let response = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .ok_or_else(|| ModelError::new("scripted model was called too often"));
        stream::once(async move { response.map(|response| ModelEvent::Completed { response }) })
            .boxed()
    }
}

struct FakeEvaluator;

struct CancelledEvaluator {
    invoked: Arc<Notify>,
}

impl EffectAdapter for CancelledEvaluator {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(async move {
            self.invoked.notify_one();
            invocation.cancellation.cancelled().await;
            EffectCompletion::OutcomeUnknown
        })
    }
}

impl EffectAdapter for FakeEvaluator {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(async move {
            let request: CodeStepRequest =
                serde_json::from_value(invocation.request).expect("step request");
            let output = match request.step {
                CodeStep::Start { source } => {
                    assert_eq!(source, "await mcp('first', {})");
                    CodeStepOutput::Suspended {
                        snapshot: "opaque-snapshot".to_owned(),
                        calls: vec![
                            CodeMcpCall {
                                call_id: 9,
                                reference: "mcp:first".to_owned(),
                                arguments: json!({"n": 1}),
                            },
                            CodeMcpCall {
                                call_id: 3,
                                reference: "mcp:second".to_owned(),
                                arguments: json!({"n": 2}),
                            },
                        ],
                    }
                }
                CodeStep::Resume { snapshot, results } => {
                    assert_eq!(snapshot, "opaque-snapshot");
                    assert_eq!(results.len(), 2);
                    assert!(
                        result_text(results.get("9").expect("first keyed result")) == "mcp:first",
                        "incorrect first keyed result"
                    );
                    assert!(
                        result_text(results.get("3").expect("second keyed result")) == "mcp:second",
                        "incorrect second keyed result"
                    );
                    CodeStepOutput::Completed {
                        result: json!({"done": true}),
                        is_error: false,
                    }
                }
            };
            EffectCompletion::from(EffectOutcome::Success(
                serde_json::to_value(output).expect("output"),
            ))
        })
    }
}

fn result_text(result: &Value) -> &str {
    result["content"][0]["text"]
        .as_str()
        .expect("MCP result text")
}

struct ConcurrentMcpTool {
    spec: ToolSpec,
    barrier: Arc<Barrier>,
    dispatched: Arc<Mutex<Vec<ToolCall>>>,
}

impl Tool for ConcurrentMcpTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.dispatched
                .lock()
                .expect("dispatch lock")
                .push(call.clone());
            self.barrier.wait().await;
            let reference = call.arguments["reference"].as_str().expect("reference");
            Ok(ToolOutput {
                content: vec![ContentBlock::text(reference)],
                details: Some(json!({"reference": reference})),
                is_error: false,
            })
        })
    }
}

struct UnknownMcpTool;

impl Tool for UnknownMcpTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "tool_execute".to_owned(),
            description: "Mock MCP executor".to_owned(),
            input_schema: json!({"type": "object"}),
        })
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let reference = call.arguments["reference"].as_str().expect("reference");
            if reference == "mcp:second" {
                Err(ToolError::outcome_unknown("MCP response was lost"))
            } else {
                Ok(ToolOutput {
                    content: vec![ContentBlock::text(reference)],
                    details: None,
                    is_error: false,
                })
            }
        })
    }
}

struct CancelledMcpTool {
    arrival: Arc<Barrier>,
}

struct OversizedMcpTool;

impl Tool for OversizedMcpTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "tool_execute".to_owned(),
            description: "Large MCP fixture".to_owned(),
            input_schema: json!({"type": "object"}),
        })
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            Ok(ToolOutput {
                content: vec![ContentBlock::text("x".repeat(2 * 1024 * 1024))],
                details: None,
                is_error: false,
            })
        })
    }
}

impl Tool for CancelledMcpTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "tool_execute".to_owned(),
            description: "Cancellation fixture".to_owned(),
            input_schema: json!({"type": "object"}),
        })
    }

    fn execute(
        &self,
        _call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.arrival.wait().await;
            cancellation.cancelled().await;
            Err(ToolError::outcome_unknown("cancelled after dispatch"))
        })
    }
}
