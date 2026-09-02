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
use crate::{ALPHA_PROFILE_ID, AgentProfileId};

#[tokio::test]
async fn trace_omits_credential_setup_and_oauth_authorization_urls() {
    let directory = tempdir().expect("temporary trace directory");
    let path = directory.path().join(TRACE_DATABASE);
    let store = TraceStore::create(path.clone(), SessionId::new(), AgentId::new(), &alpha_id())
        .expect("create trace store");
    let trace = store
        .start_run(
            CommandId::new(),
            &[ContentBlock::text("connect")],
            "provider",
            "model",
            "high",
        )
        .await
        .expect("start trace");
    let secret = "ab".repeat(32);
    trace
        .emit(AgentEvent::ToolExecutionUpdate {
            call: ToolCall {
                id: "credential-call".to_owned(),
                name: "extension_manage".to_owned(),
                arguments: json!({}),
                thought_signature: None,
                namespace: None,
            },
            update: ToolOutput {
                content: vec![ContentBlock::text(format!(
                    "{{\"status\":\"credential_required\",\"credential\":\"exa.default\",\"setup_url\":\"https://renoa.live/setup#key={secret}&token={secret}\"}}"
                ))],
                details: Some(json!({"must_not_survive": secret})),
                is_error: false,
            },
        })
        .await;
    trace
        .emit(AgentEvent::ToolExecutionUpdate {
            call: ToolCall {
                id: "oauth-call".to_owned(),
                name: "extension_manage".to_owned(),
                arguments: json!({}),
                thought_signature: None,
                namespace: None,
            },
            update: ToolOutput {
                content: vec![ContentBlock::text(format!(
                    "{{\"status\":\"authorization_required\",\"connection\":\"notion.default\",\"authorization_url\":\"https://provider.example/authorize?state={secret}\"}}"
                ))],
                details: Some(json!({"must_not_survive": secret})),
                is_error: false,
            },
        })
        .await;
    trace
        .finish("completed", None, None)
        .await
        .expect("finish trace");

    let connection = Connection::open(path).expect("open trace database");
    let mut statement = connection
        .prepare(
            "SELECT payload_json FROM events WHERE kind = 'execution_update' ORDER BY sequence",
        )
        .expect("prepare progress trace query");
    let payloads = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query progress traces")
        .collect::<Result<Vec<_>, _>>()
        .expect("load progress traces");
    assert_eq!(payloads.len(), 2);
    assert!(payloads[0].contains("credential_required"));
    assert!(payloads[0].contains("setup_url_omitted"));
    assert!(payloads[1].contains("authorization_required"));
    assert!(payloads[1].contains("authorization_url_omitted"));
    assert!(payloads.iter().all(|payload| !payload.contains(&secret)));
}

#[tokio::test]
async fn trace_records_exact_model_flow_and_normalized_usage() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let agent_id = AgentId::new();
    let profile_id = alpha_id();
    let store = TraceStore::create(path.clone(), session_id, agent_id, &profile_id)
        .expect("create trace store");
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

    assert_trace_identity(&path, session_id, agent_id, &profile_id);
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

fn assert_trace_identity(
    path: &std::path::Path,
    session_id: SessionId,
    agent_id: AgentId,
    profile_id: &AgentProfileId,
) {
    let connection = Connection::open(path).expect("open trace database");
    let stored = connection
        .query_row(
            "SELECT schema_version, session_id, agent_id, profile_id FROM trace_metadata",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .expect("read trace identity");
    assert_eq!(
        stored,
        (
            2,
            session_id.to_string(),
            agent_id.to_string(),
            profile_id.to_string()
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
    let agent_id = AgentId::new();
    let profile_id = alpha_id();
    let store = TraceStore::create(path.clone(), session_id, agent_id, &profile_id)
        .expect("create trace store");
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
    let profile_id = alpha_id();
    TraceStore::create(path.clone(), session_id, agent_id, &profile_id)
        .expect("create trace store");
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

    TraceStore::open(path.clone(), session_id, agent_id, &profile_id).expect("recover trace store");

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
fn opening_a_v1_trace_adds_agent_and_profile_identity() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let agent_id = AgentId::new();
    let profile_id = alpha_id();
    TraceStore::create(path.clone(), session_id, agent_id, &profile_id)
        .expect("create current trace store");
    let connection = Connection::open(&path).expect("open trace database");
    connection
        .execute_batch(
            "ALTER TABLE trace_metadata RENAME TO trace_metadata_v2;
             CREATE TABLE trace_metadata (
                 schema_version INTEGER PRIMARY KEY,
                 session_id TEXT NOT NULL
             ) STRICT;
             INSERT INTO trace_metadata(schema_version, session_id)
                 SELECT 1, session_id FROM trace_metadata_v2;
             DROP TABLE trace_metadata_v2;",
        )
        .expect("downgrade metadata fixture to v1");
    drop(connection);

    TraceStore::open(path.clone(), session_id, agent_id, &profile_id)
        .expect("migrate v1 trace identity");

    assert_trace_identity(&path, session_id, agent_id, &profile_id);
}

#[test]
fn trace_open_rejects_the_wrong_agent_or_profile_identity() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join(TRACE_DATABASE);
    let session_id = SessionId::new();
    let agent_id = AgentId::new();
    let profile_id = alpha_id();
    drop(
        TraceStore::create(path.clone(), session_id, agent_id, &profile_id)
            .expect("create trace store"),
    );

    assert!(matches!(
        TraceStore::open(path.clone(), session_id, AgentId::new(), &profile_id),
        Err(super::TraceError::Incompatible(_))
    ));
    let other_profile = AgentProfileId::new("renoa.other.v1").expect("valid other profile id");
    assert!(matches!(
        TraceStore::open(path, session_id, agent_id, &other_profile),
        Err(super::TraceError::Incompatible(_))
    ));
}

fn alpha_id() -> AgentProfileId {
    AgentProfileId::new(ALPHA_PROFILE_ID).expect("valid Alpha profile id")
}
