use std::fs;

use rusqlite::Connection;
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

use super::support::{AcpProcess, BRIDGE};

#[test]
fn provider_deltas_reach_the_frontend_before_the_model_finishes() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let mut process = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    process.initialize();
    let created = process.create_session(&workspace);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": "Stream" }],
            "_meta": {
                "requestId": "8a74e10d-fbe5-45fc-9412-5529336f0fdb",
                "promptId": "8a74e10d-fbe5-45fc-9412-5529336f0fdb"
            }
        }
    }));
    let first = process.read();
    assert_eq!(first["params"]["update"]["content"]["text"], "Hello ");
    let first_message_id = first["params"]["update"]["messageId"]
        .as_str()
        .expect("first assistant message id");
    Uuid::parse_str(first_message_id).expect("assistant message id is a UUID");
    assert_ne!(
        first_message_id, "8a74e10d-fbe5-45fc-9412-5529336f0fdb",
        "the frontend request identity must not become an assistant message identity"
    );
    assert!(
        !data.join("model-completed").exists(),
        "the first ACP delta was buffered until model completion"
    );

    fs::write(data.join("model-continue"), "continue").expect("release model bridge");
    let remaining = process.read_until_response(3);
    let second = &remaining[0];
    assert_eq!(second["params"]["update"]["content"]["text"], "world");
    assert_eq!(
        second["params"]["update"]["messageId"],
        first["params"]["update"]["messageId"]
    );
    let usage = remaining
        .iter()
        .find(|message| message["params"]["update"]["sessionUpdate"] == "usage_update")
        .expect("provider usage reaches ACP");
    assert_eq!(usage["params"]["update"]["used"], 3);
    assert_eq!(usage["params"]["update"]["size"], 500_000);
    let completed = remaining.last().expect("prompt response");
    assert_eq!(completed["id"], 3);
    assert_eq!(completed["result"]["stopReason"], "end_turn");
    process.finish();

    let trace = data.join("sessions").join(session_id).join("trace.sqlite3");
    assert_trace(&trace);
}

fn assert_trace(path: &std::path::Path) {
    let connection = Connection::open(path).expect("open trace database");
    let run = connection
        .query_row(
            "SELECT status, trace_complete, duration_us FROM runs",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("read completed trace run");
    assert_eq!((run.0.as_str(), run.1), ("completed", 1));
    assert!(run.2 >= 0);

    let provider_payload: String = connection
        .query_row(
            "SELECT payload_json FROM events
             WHERE component = 'model' AND kind = 'provider_request'",
            [],
            |row| row.get(0),
        )
        .expect("read exact provider request");
    let provider_payload: serde_json::Value =
        serde_json::from_str(&provider_payload).expect("decode provider payload");
    assert_eq!(provider_payload["model"], "grok-test");
    assert!(provider_payload["messages"].is_array());
    assert_eq!(
        provider_payload["tools"]
            .as_array()
            .expect("tool array")
            .len(),
        6
    );

    let response: (String, i64) = connection
        .query_row(
            "SELECT status, occurred_at_ms FROM events
             WHERE component = 'model' AND kind = 'provider_response'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read provider response");
    assert_eq!(response.0, "200");
    assert!(response.1 > 0);

    let completion = connection
        .query_row(
            "SELECT input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    duration_us, time_to_first_output_us
             FROM events WHERE component = 'model' AND kind = 'request_finished'",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .expect("read normalized model completion");
    assert_eq!(
        (completion.0, completion.1, completion.2, completion.3),
        (1, 2, 0, 0)
    );
    assert!(completion.4 >= 0);
    assert!(completion.5 >= 0);

    let chunks: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM events
             WHERE component = 'model' AND kind = 'stream_chunk'",
            [],
            |row| row.get(0),
        )
        .expect("count model chunks");
    assert_eq!(chunks, 2);
}
