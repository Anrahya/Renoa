use std::collections::BTreeMap;

use renoa_agent::{
    AgentEvent, AgentEventSink as _, AssistantDelta, AssistantMetadata, ContentBlock, ModelRequest,
    ModelResponse, StopReason, TokenUsage, ToolCall, ToolOutput,
};
use renoa_kernel::{AgentId, CommandId, SessionId};
use rusqlite::Connection;
use serde_json::json;
use tempfile::tempdir;

use super::{TRACE_DATABASE, TraceStore};

/// Every event of a run that carries `secret` in its content.
async fn emit_content(trace: &super::TraceRun, secret: &str) {
    let call = ToolCall {
        id: "call".to_owned(),
        name: "plugin_manage".to_owned(),
        arguments: json!({ "argument": secret }),
        thought_signature: None,
        namespace: None,
    };
    let output = ToolOutput {
        content: vec![ContentBlock::text(secret)],
        details: Some(json!({ "details": secret })),
        is_error: false,
    };
    for event in [
        AgentEvent::ModelRequestStart {
            invocation_id: "model".to_owned(),
            request: ModelRequest {
                system_prompt: secret.to_owned(),
                messages: vec![renoa_agent::Message::user_text(secret)],
                tools: Vec::new(),
            },
        },
        AgentEvent::ModelProviderRequest {
            invocation_id: "model".to_owned(),
            payload: json!({ "messages": [secret] }),
        },
        AgentEvent::MessageUpdate {
            content_index: 0,
            delta: AssistantDelta::Text {
                text: secret.to_owned(),
            },
        },
        AgentEvent::ModelRequestChunk {
            invocation_id: "model".to_owned(),
            content_index: 0,
            delta: AssistantDelta::Text {
                text: secret.to_owned(),
            },
        },
        AgentEvent::ModelRequestEnd {
            invocation_id: "model".to_owned(),
            response: ModelResponse {
                content: vec![renoa_agent::AssistantContent::text(secret)],
                stop_reason: StopReason::Stop,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
        },
        AgentEvent::ToolExecutionStart { call: call.clone() },
        AgentEvent::ToolExecutionUpdate {
            call: call.clone(),
            update: output,
        },
        AgentEvent::ToolExecutionEnd {
            call: call.clone(),
            result: renoa_agent::ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                content: vec![ContentBlock::text(secret)],
                details: Some(json!({ "details": secret })),
                is_error: false,
            },
        },
    ] {
        trace.emit(event).await;
    }
}

#[tokio::test]
async fn a_trace_records_what_happened_and_no_content() {
    let directory = tempdir().expect("temporary trace directory");
    let path = directory.path().join(TRACE_DATABASE);
    let store = TraceStore::create(path.clone(), SessionId::new(), AgentId::new())
        .expect("create trace store");
    let secret = "private conversation text";
    let trace = store
        .start_run(
            CommandId::new(),
            &[ContentBlock::text(secret)],
            "provider",
            "model",
            "high",
        )
        .await
        .expect("start trace");
    emit_content(&trace, secret).await;
    trace
        .finish("completed", None, None)
        .await
        .expect("finish trace");

    let connection = Connection::open(path).expect("open trace database");
    let rows = connection
        .prepare("SELECT kind || ' ' || payload_json FROM events ORDER BY sequence")
        .expect("prepare")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    assert_eq!(
        rows,
        [
            r#"request_started {"messages":1,"tools":0}"#.to_owned(),
            format!(
                r#"provider_request {{"bytes":{}}}"#,
                json!({ "messages": [secret] }).to_string().len()
            ),
            r#"request_finished {"stop_reason":"stop"}"#.to_owned(),
            "execution_started null".to_owned(),
            "execution_finished null".to_owned(),
        ],
        "streamed pieces and tool progress leave no rows"
    );
    let input: String = connection
        .query_row("SELECT input_json FROM runs", [], |row| row.get(0))
        .expect("run input");
    assert!(!input.contains(secret), "{input}");
}

#[tokio::test]
async fn a_failed_tool_call_keeps_the_start_of_its_error() {
    let directory = tempdir().expect("temporary trace directory");
    let path = directory.path().join(TRACE_DATABASE);
    let store = TraceStore::create(path.clone(), SessionId::new(), AgentId::new())
        .expect("create trace store");
    let trace = store
        .start_run(CommandId::new(), &[], "provider", "model", "high")
        .await
        .expect("start trace");
    let error = format!("missing-notes.txt: No such file{}", "!".repeat(600));
    trace
        .emit(AgentEvent::ToolExecutionEnd {
            call: ToolCall {
                id: "read".to_owned(),
                name: "read_file".to_owned(),
                arguments: json!({}),
                thought_signature: None,
                namespace: None,
            },
            result: renoa_agent::ToolResult {
                call_id: "read".to_owned(),
                name: "read_file".to_owned(),
                content: vec![ContentBlock::text(error.clone())],
                details: None,
                is_error: true,
            },
        })
        .await;
    trace
        .finish("completed", None, None)
        .await
        .expect("finish trace");

    let (status, payload) = Connection::open(path)
        .expect("open trace database")
        .query_row(
            "SELECT status, payload_json FROM events WHERE kind = 'execution_finished'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("tool row");
    assert_eq!(status, "failed");
    assert_eq!(
        payload,
        json!({ "error": error.chars().take(500).collect::<String>() }).to_string()
    );
}

#[tokio::test]
async fn trace_records_model_timing_and_normalized_usage() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let agent_id = AgentId::new();
    let store = TraceStore::create(path.clone(), session_id, agent_id).expect("create trace store");
    let request = ModelRequest {
        system_prompt: "Be exact.".to_owned(),
        messages: vec![renoa_agent::Message::user_text("Inspect this project")],
        tools: Vec::new(),
    };
    let response = ModelResponse {
        content: vec![renoa_agent::AssistantContent::text("Done")],
        stop_reason: StopReason::Stop,
        usage: Some(TokenUsage {
            input: 10,
            output: 2,
            cache_read: 7,
            cache_write: 1,
        }),
        metadata: AssistantMetadata::default(),
    };
    let trace = store
        .start_run(
            CommandId::new(),
            &[ContentBlock::text("Inspect this project")],
            "xai",
            "grok-code",
            "high",
        )
        .await
        .expect("start trace");
    let invocation_id = "model-call-1".to_owned();

    trace
        .emit(AgentEvent::ModelRequestStart {
            invocation_id: invocation_id.clone(),
            request: request.clone(),
        })
        .await;
    trace
        .emit(AgentEvent::ModelProviderRequest {
            invocation_id: invocation_id.clone(),
            payload: json!({ "model": "grok-code", "messages": ["exact payload"] }),
        })
        .await;
    trace
        .emit(AgentEvent::ModelRetryAttempt {
            invocation_id: invocation_id.clone(),
            attempt: 1,
            next_attempt: 2,
            category: renoa_agent::ModelErrorKind::Network,
            delay_ms: 250,
            cause_code: Some("ECONNRESET".to_owned()),
        })
        .await;
    trace
        .emit(AgentEvent::ModelProviderResponse {
            invocation_id: invocation_id.clone(),
            status: 200,
            headers: BTreeMap::from([("x-request-id".to_owned(), "request-1".to_owned())]),
        })
        .await;
    trace
        .emit(AgentEvent::ModelRequestChunk {
            invocation_id: invocation_id.clone(),
            content_index: 0,
            delta: AssistantDelta::Text {
                text: "Done".to_owned(),
            },
        })
        .await;
    trace
        .emit(AgentEvent::ModelRequestEnd {
            invocation_id,
            response,
        })
        .await;
    trace
        .finish("completed", None, None)
        .await
        .expect("finish trace");
    drop(trace);

    assert_trace_identity(&path, session_id, agent_id);
    assert_run_metadata(&path);
    assert_model_diagnostics(&path);
}

fn assert_run_metadata(path: &std::path::Path) {
    let connection = Connection::open(path).expect("open trace database");
    let run = connection
        .query_row(
            "SELECT status, trace_complete, provider, model, reasoning FROM runs",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .expect("read run");
    assert_eq!(
        run,
        (
            "completed".to_owned(),
            1,
            "xai".to_owned(),
            "grok-code".to_owned(),
            "high".to_owned()
        )
    );
}

fn assert_trace_identity(path: &std::path::Path, session_id: SessionId, agent_id: AgentId) {
    let connection = Connection::open(path).expect("open trace database");
    let stored = connection
        .query_row(
            "SELECT schema_version, session_id, agent_id FROM trace_metadata",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .expect("read trace identity");
    assert_eq!(stored, (4, session_id.to_string(), agent_id.to_string()));
}

fn assert_model_diagnostics(path: &std::path::Path) {
    let connection = Connection::open(path).expect("open trace database");
    let finished = connection
        .query_row(
            "SELECT input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    duration_us, time_to_first_output_us, payload_json
             FROM events WHERE kind = 'request_finished'",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .expect("read model completion");
    assert_eq!(
        (finished.0, finished.1, finished.2, finished.3),
        (10, 2, 7, 1)
    );
    assert!(finished.4 >= 0);
    assert!(finished.5 >= 0);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&finished.6).expect("response JSON"),
        json!({ "stop_reason": "stop" })
    );
    let provider_payload: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'provider_request'",
            [],
            |row| row.get(0),
        )
        .expect("read provider payload");
    assert_eq!(
        provider_payload,
        json!({ "bytes": json!({ "model": "grok-code", "messages": ["exact payload"] }).to_string().len() })
            .to_string()
    );
    let retry_payload: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'retry_attempt'",
            [],
            |row| row.get(0),
        )
        .expect("read retry diagnostic");
    assert!(retry_payload.contains("\"attempt\":1"));
    assert!(retry_payload.contains("ECONNRESET"));
}

#[tokio::test]
async fn dropping_an_unfinished_trace_marks_it_interrupted() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let agent_id = AgentId::new();
    let store = TraceStore::create(path.clone(), session_id, agent_id).expect("create trace store");
    let trace = store
        .start_run(
            CommandId::new(),
            &[ContentBlock::text("start")],
            "xai",
            "grok-code",
            "high",
        )
        .await
        .expect("start trace");

    drop(trace);

    let connection = Connection::open(path).expect("open trace database");
    let run = connection
        .query_row("SELECT status, trace_complete FROM runs", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .expect("read interrupted run");
    assert_eq!(run, ("interrupted".to_owned(), 0));
}

#[test]
fn opening_a_trace_repairs_a_run_left_running_by_process_loss() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let agent_id = AgentId::new();
    TraceStore::create(path.clone(), session_id, agent_id).expect("create trace store");
    let connection = Connection::open(&path).expect("open trace database");
    connection
        .execute(
            "INSERT INTO runs(
                run_id, session_id, command_id, started_at_ms, status, trace_complete,
                provider, model, reasoning, input_json
             ) VALUES (?1, ?2, ?3, 1, 'running', 0, 'xai', 'grok', 'high', '[]')",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                session_id.to_string(),
                CommandId::new().to_string(),
            ],
        )
        .expect("insert interrupted run");
    drop(connection);

    TraceStore::open(path.clone(), session_id, agent_id).expect("recover trace store");

    let connection = Connection::open(path).expect("reopen trace database");
    let run = connection
        .query_row(
            "SELECT status, trace_complete, error_code, duration_us FROM runs",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .expect("read recovered run");
    assert_eq!(run.0, "interrupted");
    assert_eq!(run.1, 0);
    assert_eq!(run.2, "trace_owner_interrupted");
    assert!(run.3 >= 0);
}

#[test]
fn opening_a_trace_from_an_older_runtime_is_rejected() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let agent_id = AgentId::new();
    TraceStore::create(path.clone(), session_id, agent_id).expect("create current trace store");
    let connection = Connection::open(&path).expect("open trace database");
    connection
        .execute("UPDATE trace_metadata SET schema_version = 2", [])
        .expect("construct older schema fixture");
    drop(connection);

    assert!(matches!(
        TraceStore::open(path, session_id, agent_id),
        Err(super::TraceError::Incompatible(_))
    ));
}

#[test]
fn trace_open_rejects_the_wrong_session_or_agent_identity() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let agent_id = AgentId::new();
    drop(TraceStore::create(path.clone(), session_id, agent_id).expect("create trace store"));

    assert!(matches!(
        TraceStore::open(path.clone(), SessionId::new(), agent_id),
        Err(super::TraceError::Incompatible(_))
    ));
    assert!(matches!(
        TraceStore::open(path, session_id, AgentId::new()),
        Err(super::TraceError::Incompatible(_))
    ));
}

#[tokio::test]
async fn a_schema_3_trace_loses_its_content_and_gives_the_space_back() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let agent_id = AgentId::new();
    let store = TraceStore::create(path.clone(), session_id, agent_id).expect("create trace");
    let trace = store
        .start_run(CommandId::new(), &[], "provider", "model", "high")
        .await
        .expect("start trace");
    trace
        .finish("completed", None, None)
        .await
        .expect("finish trace");
    // What a schema 3 runtime wrote: the full request, streamed pieces and
    // tool output.
    let secret = "x".repeat(1_000_000);
    let connection = Connection::open(&path).expect("open trace database");
    let run_id: String = connection
        .query_row("SELECT run_id FROM runs", [], |row| row.get(0))
        .expect("run");
    for (sequence, kind) in [
        "request_started",
        "provider_request",
        "stream_chunk",
        "chunk",
        "execution_update",
        "execution_finished",
        "request_finished",
    ]
    .into_iter()
    .enumerate()
    {
        connection
            .execute(
                "INSERT INTO events(run_id, sequence, occurred_at_ms, elapsed_us, component,
                    kind, payload_json) VALUES (?1, ?2, 0, 0, 'model', ?3, ?4)",
                rusqlite::params![
                    run_id,
                    i64::try_from(sequence).expect("sequence") + 1,
                    kind,
                    json!([secret]).to_string()
                ],
            )
            .expect("seed schema 3 event");
    }
    connection
        .execute_batch(&format!(
            "UPDATE runs SET input_json = '{}';
             UPDATE trace_metadata SET schema_version = 3;
             PRAGMA wal_checkpoint(TRUNCATE);",
            json!([secret])
        ))
        .expect("schema 3 shape");
    drop(connection);
    let before = std::fs::metadata(&path).expect("size").len();

    TraceStore::open(path.clone(), session_id, agent_id).expect("upgrade schema 3");
    TraceStore::open(path.clone(), session_id, agent_id).expect("reopen schema 4");
    assert_trace_identity(&path, session_id, agent_id);
    let connection = Connection::open(&path).expect("open upgraded trace");
    let kinds = connection
        .prepare("SELECT kind || ' ' || payload_json FROM events ORDER BY sequence")
        .expect("prepare")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    assert_eq!(
        kinds,
        [
            "request_started null",
            "provider_request null",
            "execution_finished null",
            "request_finished null"
        ]
    );
    let input: String = connection
        .query_row("SELECT input_json FROM runs", [], |row| row.get(0))
        .expect("run input");
    assert_eq!(input, "null");
    let after = std::fs::metadata(&path).expect("size").len();
    assert!(after * 10 < before, "{before} bytes became {after}");
}
