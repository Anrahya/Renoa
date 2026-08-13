use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Message, ModelResponse, StopReason, Tool,
    ToolCall, ToolOutput, ToolResult, ToolSpec,
};
use renoa_harness::{
    Harness, OperationOutcome, OperationRequest, RequestId, RunNext, RuntimeProfile, SessionId,
    ToolBinding, ToolRecovery,
};
use tempfile::tempdir;

mod tool_support;

use tool_support::{RecordingTool, ScriptedModel, usage};

#[tokio::test]
async fn a_tool_result_is_committed_before_the_model_continues() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
        thought_signature: None,
        namespace: None,
    };
    let first_response = ModelResponse {
        content: vec![AssistantContent::tool_call(call.clone())],
        stop_reason: StopReason::ToolUse,
        usage: Some(usage(4, 2)),
        metadata: AssistantMetadata::default(),
    };
    let final_response = ModelResponse {
        content: vec![AssistantContent::text("implemented")],
        stop_reason: StopReason::Stop,
        usage: Some(usage(6, 3)),
        metadata: AssistantMetadata::default(),
    };
    let model = Arc::new(ScriptedModel::new([
        first_response.clone(),
        final_response.clone(),
    ]));
    let tool = Arc::new(RecordingTool::new(
        ToolSpec {
            name: "read_file".to_owned(),
            description: "Read one file".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ToolOutput {
            content: vec![ContentBlock::text("pub fn existing() {}")],
            details: Some(serde_json::json!({"bytes": 20})),
        },
    ));
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(4).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(tool.clone(), ToolRecovery::SafeToReplay)],
        NonZeroU32::new(8).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("fix it")]),
        )
        .await
        .expect("admit operation");

    assert_eq!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("run operation"),
        RunNext::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Completed {
                output: "implemented".to_owned(),
                stop_reason: StopReason::Stop,
                usage: Some(usage(10, 5)),
            },
        }
    );
    let tool_result = ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text("pub fn existing() {}")],
        details: Some(serde_json::json!({"bytes": 20})),
        is_error: false,
    };
    assert_model_continuation(&model, &tool, &call, &first_response, &tool_result);

    drop(harness);
    assert_persisted_transcript(
        &database,
        session_id,
        first_response,
        final_response,
        tool_result,
    )
    .await;
}

fn assert_model_continuation(
    model: &ScriptedModel,
    tool: &RecordingTool,
    call: &ToolCall,
    first_response: &ModelResponse,
    tool_result: &ToolResult,
) {
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].tools, vec![tool.spec().clone()]);
    assert_eq!(
        requests[1].messages,
        vec![
            Message::user_text("fix it"),
            Message::Assistant {
                content: first_response.content.clone(),
                stop_reason: first_response.stop_reason,
                usage: first_response.usage,
                metadata: first_response.metadata.clone(),
            },
            Message::Tool {
                result: tool_result.clone(),
            },
        ]
    );
    assert_eq!(tool.calls(), vec![call.clone()]);
}

async fn assert_persisted_transcript(
    database: &std::path::Path,
    session_id: SessionId,
    first_response: ModelResponse,
    final_response: ModelResponse,
    tool_result: ToolResult,
) {
    let harness = Harness::open(database).expect("reopen harness");
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect session")
            .messages,
        vec![
            Message::user_text("fix it"),
            Message::Assistant {
                content: first_response.content,
                stop_reason: first_response.stop_reason,
                usage: first_response.usage,
                metadata: first_response.metadata,
            },
            Message::Tool {
                result: tool_result,
            },
            Message::Assistant {
                content: final_response.content,
                stop_reason: final_response.stop_reason,
                usage: final_response.usage,
                metadata: final_response.metadata,
            },
        ]
    );
}

#[tokio::test]
async fn an_unavailable_tool_becomes_a_model_visible_error_without_an_effect() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = ToolCall {
        id: "call-missing".to_owned(),
        name: "missing".to_owned(),
        arguments: serde_json::json!({}),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new([
        ModelResponse {
            content: vec![AssistantContent::tool_call(call.clone())],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
        ModelResponse {
            content: vec![AssistantContent::text("recovered")],
            stop_reason: StopReason::Stop,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
    ]));
    let available = Arc::new(RecordingTool::new(
        ToolSpec {
            name: "available".to_owned(),
            description: "Available tool".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        ToolOutput {
            content: vec![],
            details: None,
        },
    ));
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(3).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(
            available.clone(),
            ToolRecovery::NeverReplay,
        )],
        NonZeroU32::new(4).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("continue")]),
        )
        .await
        .expect("admit operation");

    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("complete operation"),
        RunNext::Finished {
            outcome: OperationOutcome::Completed { ref output, .. },
            ..
        } if output == "recovered"
    ));
    assert!(available.calls().is_empty());
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].messages.last(),
        Some(&Message::Tool {
            result: ToolResult {
                call_id: call.id,
                name: call.name,
                content: vec![ContentBlock::text("Tool `missing` is not available.")],
                details: None,
                is_error: true,
            },
        })
    );
}

#[tokio::test]
async fn a_length_truncated_tool_call_is_never_executed() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = ToolCall {
        id: "call-truncated".to_owned(),
        name: "write_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs", "content": "partial"}),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new([
        ModelResponse {
            content: vec![AssistantContent::tool_call(call.clone())],
            stop_reason: StopReason::Length,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
        ModelResponse {
            content: vec![AssistantContent::text("finished safely")],
            stop_reason: StopReason::Stop,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
    ]));
    let tool = Arc::new(RecordingTool::new(
        ToolSpec {
            name: call.name.clone(),
            description: "Write one file".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        ToolOutput {
            content: vec![],
            details: None,
        },
    ));
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(3).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(tool.clone(), ToolRecovery::NeverReplay)],
        NonZeroU32::new(4).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("write it")]),
        )
        .await
        .expect("admit operation");

    harness
        .run_next(session_id, &profile)
        .await
        .expect("complete operation");
    assert!(tool.calls().is_empty());
    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::Tool {
            result: ToolResult {
                call_id: call.id,
                name: call.name,
                content: vec![ContentBlock::text(
                    "Tool call was not executed because the model response reached its token limit.",
                )],
                details: None,
                is_error: true,
            },
        })
    );
}

#[tokio::test]
async fn a_tool_batch_executes_and_enters_context_in_source_order() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let calls = ["first", "second"].map(|id| ToolCall {
        id: id.to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": id}),
        thought_signature: None,
        namespace: None,
    });
    let model = Arc::new(ScriptedModel::new([
        ModelResponse {
            content: calls
                .iter()
                .cloned()
                .map(AssistantContent::tool_call)
                .collect(),
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
        ModelResponse {
            content: vec![AssistantContent::text("done")],
            stop_reason: StopReason::Stop,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
    ]));
    let tool = Arc::new(RecordingTool::new(
        ToolSpec {
            name: "read_file".to_owned(),
            description: "Read one file".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        ToolOutput {
            content: vec![ContentBlock::text("contents")],
            details: None,
        },
    ));
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(3).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(tool.clone(), ToolRecovery::NeverReplay)],
        NonZeroU32::new(4).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect both")]),
        )
        .await
        .expect("admit operation");

    harness
        .run_next(session_id, &profile)
        .await
        .expect("complete operation");
    assert_eq!(tool.calls(), calls);
    let requests = model.requests();
    let tool_messages = requests[1]
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { result } => Some(result.call_id.as_str()),
            Message::User { .. } | Message::Assistant { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_messages, vec!["first", "second"]);
}
