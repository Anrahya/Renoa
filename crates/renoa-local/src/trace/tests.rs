use std::collections::BTreeMap;

use renoa_agent::{
    AgentEvent, AgentEventSink as _, AssistantDelta, AssistantMetadata, ContentBlock, ModelRequest,
    ModelResponse, StopReason, TokenUsage,
};
use renoa_kernel::{CommandId, SessionId};
use rusqlite::Connection;
use serde_json::json;
use tempfile::tempdir;

use super::{TRACE_DATABASE, TraceStore};

#[tokio::test]
async fn trace_records_exact_model_flow_and_normalized_usage() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let store = TraceStore::create(path.clone(), session_id).expect("create trace store");
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
        serde_json::to_value(ModelResponse {
            content: vec![renoa_agent::AssistantContent::text("Done")],
            stop_reason: StopReason::Stop,
            usage: Some(TokenUsage {
                input: 10,
                output: 2,
                cache_read: 7,
                cache_write: 1,
            }),
            metadata: AssistantMetadata::default(),
        })
        .expect("encode expected response")
    );
    let provider_payload: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'provider_request'",
            [],
            |row| row.get(0),
        )
        .expect("read provider payload");
    assert!(provider_payload.contains("exact payload"));
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
    let store = TraceStore::create(path.clone(), session_id).expect("create trace store");
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
    TraceStore::create(path.clone(), session_id).expect("create trace store");
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

    TraceStore::open(path.clone(), session_id).expect("recover trace store");

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
