use std::{
    collections::{BTreeMap, VecDeque},
    num::NonZeroU32,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt as _, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, Model, ModelError,
    ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall,
    ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, AgentToolBinding, CodeModeBinding, CodeStep, CodeStepOutput,
    CodeStepRequest, ContextBinding, ModelBinding, build_runtime_with_code_mode,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, EventCursor, Kernel,
    OperationOutcome, SessionId,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{EvaluationError, MontyEvaluator};

#[test]
fn rejects_an_unpinned_worker_before_host_state_is_created() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let worker = directory.path().join("wrong-worker");
    std::fs::write(&worker, b"not the exact Monty binary").expect("write fixture");
    assert!(MontyEvaluator::new(&worker).is_err());
}

#[tokio::test]
async fn exact_worker_suspends_parallel_calls_and_restores_into_a_fresh_pool() {
    let Some(worker) = std::env::var_os("RENOA_TEST_MONTY_WORKER").map(PathBuf::from) else {
        return;
    };
    let first = MontyEvaluator::new(&worker).expect("verify exact worker");
    let start = first.evaluate(CodeStepRequest {
        run_id: "test-parallel".to_owned(),
        step: CodeStep::Start {
            source: "import asyncio\nresults = await asyncio.gather(mcp('mcp:first', {'n': 1}), mcp('mcp:second', {'n': 2}))\nresults".to_owned(),
        },
    }).await.expect("evaluate start");
    let CodeStepOutput::Suspended { snapshot, calls } = start else {
        panic!("parallel MCP calls did not suspend")
    };
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].reference, "mcp:first");
    assert_eq!(calls[1].reference, "mcp:second");
    assert_ne!(calls[0].call_id, calls[1].call_id);
    let mut results = BTreeMap::new();
    results.insert(calls[1].call_id.to_string(), json!({"value": "second"}));
    results.insert(calls[0].call_id.to_string(), json!({"value": "first"}));
    let second = MontyEvaluator::new(&worker).expect("fresh worker pool");
    let completed = second
        .evaluate(CodeStepRequest {
            run_id: "test-parallel".to_owned(),
            step: CodeStep::Resume { snapshot, results },
        })
        .await
        .expect("restore and resume");
    assert_eq!(
        completed,
        CodeStepOutput::Completed {
            result: json!([{"value": "first"}, {"value": "second"}]),
            is_error: false,
        }
    );
}

#[tokio::test]
async fn exact_worker_resumes_dependent_mcp_waves_across_fresh_pools() {
    let Some(worker) = std::env::var_os("RENOA_TEST_MONTY_WORKER").map(PathBuf::from) else {
        return;
    };
    let source = "first = await mcp('mcp:first', {'seed': 1})\nsecond = await mcp('mcp:second', {'from': first['value']})\nsecond";
    let first = MontyEvaluator::new(&worker).expect("verify exact worker");
    let start = first
        .evaluate(CodeStepRequest {
            run_id: "test-dependent".to_owned(),
            step: CodeStep::Start {
                source: source.to_owned(),
            },
        })
        .await
        .expect("first wave");
    let CodeStepOutput::Suspended {
        snapshot: first_snapshot,
        calls: first_calls,
    } = start
    else {
        panic!("first MCP call did not suspend")
    };
    assert_eq!(first_calls.len(), 1);
    assert_eq!(first_calls[0].reference, "mcp:first");
    let first_results = BTreeMap::from([(
        first_calls[0].call_id.to_string(),
        json!({"value": "from-first"}),
    )]);
    let second = MontyEvaluator::new(&worker).expect("fresh second pool");
    let next = second
        .evaluate(CodeStepRequest {
            run_id: "test-dependent".to_owned(),
            step: CodeStep::Resume {
                snapshot: first_snapshot,
                results: first_results,
            },
        })
        .await
        .expect("second wave");
    let CodeStepOutput::Suspended {
        snapshot: second_snapshot,
        calls: second_calls,
    } = next
    else {
        panic!("dependent MCP call did not suspend")
    };
    assert_eq!(second_calls.len(), 1);
    assert_eq!(second_calls[0].reference, "mcp:second");
    assert_eq!(second_calls[0].arguments, json!({"from": "from-first"}));
    let second_results = BTreeMap::from([(
        second_calls[0].call_id.to_string(),
        json!({"value": "done"}),
    )]);
    let third = MontyEvaluator::new(&worker).expect("fresh third pool");
    let completed = third
        .evaluate(CodeStepRequest {
            run_id: "test-dependent".to_owned(),
            step: CodeStep::Resume {
                snapshot: second_snapshot,
                results: second_results,
            },
        })
        .await
        .expect("completed script");
    assert_eq!(
        completed,
        CodeStepOutput::Completed {
            result: json!({"value": "done"}),
            is_error: false,
        }
    );
}

#[tokio::test]
async fn exact_worker_resolves_separately_awaited_futures() {
    let Some(worker) = std::env::var_os("RENOA_TEST_MONTY_WORKER").map(PathBuf::from) else {
        return;
    };
    let evaluator = MontyEvaluator::new(&worker).expect("verify exact worker");
    let output = evaluator
        .evaluate(CodeStepRequest {
            run_id: "test-separate-awaits".to_owned(),
            step: CodeStep::Start {
                source: "first = mcp('mcp:first', {})\nsecond = mcp('mcp:second', {})\na = await first\nb = await second\n[a, b]".to_owned(),
            },
        })
        .await
        .expect("separately awaited calls");
    let CodeStepOutput::Suspended { calls, .. } = output else {
        panic!("separately awaited calls did not suspend")
    };
    assert_eq!(calls.len(), 2);
}

#[tokio::test]
async fn exact_worker_does_not_silently_drop_unawaited_mcp_calls() {
    let Some(worker) = std::env::var_os("RENOA_TEST_MONTY_WORKER").map(PathBuf::from) else {
        return;
    };
    let evaluator = MontyEvaluator::new(&worker).expect("verify exact worker");
    let result = evaluator
        .evaluate(CodeStepRequest {
            run_id: "test-unawaited".to_owned(),
            step: CodeStep::Start {
                source: "mcp('mcp:orphan', {})\n'finished'".to_owned(),
            },
        })
        .await;
    assert!(
        matches!(result, Err(EvaluationError::User(message)) if message.contains("must be awaited"))
    );
}

#[tokio::test]
async fn exact_worker_runs_parallel_mcp_effects_through_the_kernel() {
    let Some(worker) = std::env::var_os("RENOA_TEST_MONTY_WORKER").map(PathBuf::from) else {
        return;
    };
    let directory = tempfile::tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent = AgentId::new();
    let session = SessionId::new();
    kernel.create_agent(agent).expect("create agent");
    kernel
        .create_session(session, agent)
        .expect("create session");
    let source = "import asyncio\nresults = await asyncio.gather(mcp('mcp:first', {'n': 1}), mcp('mcp:second', {'n': 2}))\n[results[0]['content'][0]['text'], results[1]['content'][0]['text']]";
    let model = Arc::new(EndToEndModel {
        responses: Mutex::new(VecDeque::from([
            ModelResponse {
                content: vec![AssistantContent::tool_call(ToolCall {
                    id: "outer".to_owned(),
                    name: "code_mode".to_owned(),
                    arguments: json!({"source": source}),
                    thought_signature: None,
                    namespace: None,
                })],
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
    });
    let runtime = build_runtime_with_code_mode(
        AgentLoopConfig::new(
            "Use Code Mode",
            NonZeroU32::new(3).expect("nonzero"),
            NonZeroU32::new(3).expect("nonzero"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("scripted-v1", model, EffectRecovery::SafeToReplay),
        Vec::new(),
        CodeModeBinding::new(
            MontyEvaluator::revision(),
            Arc::new(MontyEvaluator::new(&worker).expect("pinned worker")),
            AgentToolBinding::new(
                "test-mcp-v1",
                Arc::new(EndToEndMcpTool),
                EffectRecovery::NeverReplay,
            ),
        ),
    )
    .expect("build runtime");
    let operation = kernel
        .submit(
            session,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text("Use both MCP tools.")).expect("command"),
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
    assert_eq!(snapshot.operations[0].effect_batches.len(), 5);
    assert_eq!(snapshot.operations[0].effect_batches[2].effects.len(), 2);
    let messages = kernel
        .events_after(session, EventCursor::START)
        .expect("events")
        .events
        .into_iter()
        .map(|event| serde_json::from_value::<Message>(event.payload).expect("message"))
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 4);
    let Message::Tool { result } = &messages[2] else {
        panic!("outer Code Mode result is missing")
    };
    assert_eq!(result.call_id, "outer");
    assert!(!result.is_error);
    assert_eq!(
        result.content,
        vec![ContentBlock::text("[\"mcp:first\",\"mcp:second\"]")]
    );
}

struct EndToEndModel {
    responses: Mutex<VecDeque<ModelResponse>>,
}

impl Model for EndToEndModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let response = self
            .responses
            .lock()
            .expect("model responses")
            .pop_front()
            .ok_or_else(|| ModelError::new("model was called too many times"));
        stream::once(async move { response.map(|response| ModelEvent::Completed { response }) })
            .boxed()
    }
}

struct EndToEndMcpTool;

impl Tool for EndToEndMcpTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "tool_execute".to_owned(),
            description: "MCP fixture".to_owned(),
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
            let reference = call.arguments["reference"].as_str().expect("MCP reference");
            Ok(ToolOutput {
                content: vec![ContentBlock::text(reference)],
                details: None,
                is_error: false,
            })
        })
    }
}
