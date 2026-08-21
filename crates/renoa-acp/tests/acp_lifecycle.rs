#[path = "acp_lifecycle/close.rs"]
mod close;
#[path = "acp_lifecycle/observability.rs"]
mod observability;
mod support;

use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};

use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

use support::{AcpProcess, BRIDGE};

#[test]
fn a_frontend_can_create_and_run_one_durable_session() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let mut process = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);

    let initialized = process.initialize();
    assert_eq!(initialized["id"], 1);
    assert_eq!(initialized["result"]["protocolVersion"], 1);
    assert_eq!(
        initialized["result"]["agentCapabilities"]["loadSession"],
        true
    );
    assert_eq!(
        initialized["result"]["agentCapabilities"]["promptCapabilities"]["image"],
        true
    );
    assert!(initialized["result"]["agentCapabilities"]["sessionCapabilities"]["close"].is_object());

    let created = process.create_session(&workspace);
    assert_eq!(created["id"], 2);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();

    let turn_id = "fb715099-f88f-4559-b4e6-9f8f2ef57282";
    let (update, completed) = process.prompt(&session_id, "Hello", turn_id);
    assert_eq!(update["method"], "session/update");
    assert_eq!(update["params"]["sessionId"], session_id);
    assert_eq!(
        update["params"]["update"],
        json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "Hello back." },
            "messageId": turn_id
        })
    );
    assert_eq!(completed["id"], 3);
    assert_eq!(completed["result"]["stopReason"], "end_turn");

    process.finish();
    assert!(
        data.join("sessions")
            .join(session_id)
            .join("kernel.sqlite3")
            .is_file(),
        "the ACP response was sent without durable session state"
    );
}

#[test]
fn image_prompt_content_crosses_acp_without_being_changed() {
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
            "prompt": [
                { "type": "text", "text": "Image" },
                { "type": "image", "data": "AAEC", "mimeType": "image/png" }
            ],
            "_meta": {
                "requestId": "ba303fbd-c106-4a98-b613-bf6007f42f13",
                "promptId": "ba303fbd-c106-4a98-b613-bf6007f42f13"
            }
        }
    }));
    let update = process.read();
    let completed = process.read();
    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Image received."
    );
    assert_eq!(completed["result"]["stopReason"], "end_turn");
    process.finish();
}

#[test]
fn a_new_process_resumes_the_same_durable_conversation() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");

    let mut first = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    first.initialize();
    let created = first.create_session(&workspace);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let (first_update, first_response) =
        first.prompt(&session_id, "First", "a19115d8-2796-496a-8763-abe0159efd24");
    assert_eq!(
        first_update["params"]["update"]["content"]["text"],
        "First response."
    );
    assert_eq!(first_response["result"]["stopReason"], "end_turn");
    first.finish();

    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    let (history, loaded) = resumed.load_session(&workspace, &session_id);
    assert_eq!(loaded["id"], 2);
    assert!(loaded["result"].is_object());
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[0]["params"]["update"]["sessionUpdate"],
        "user_message_chunk"
    );
    assert_eq!(history[0]["params"]["update"]["content"]["text"], "First");
    assert_eq!(
        history[1]["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert_eq!(
        history[1]["params"]["update"]["content"]["text"],
        "First response."
    );
    for update in &history {
        let message_id = update["params"]["update"]["messageId"]
            .as_str()
            .expect("durable message id");
        Uuid::parse_str(message_id).expect("message id is a durable event UUID");
    }

    let (update, response) = resumed.prompt(
        &session_id,
        "Second",
        "9d3c5140-6c66-48ea-aa30-1a9329d20da6",
    );
    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Continued from durable history."
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");
    resumed.finish();
}

#[test]
fn session_cancel_stops_the_active_model_process() {
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
            "prompt": [{ "type": "text", "text": "Wait" }],
            "_meta": {
                "requestId": "467ffcc5-b743-466d-9717-af8112444910",
                "promptId": "467ffcc5-b743-466d-9717-af8112444910"
            }
        }
    }));
    wait_for_path(&data.join("model-started"));
    let cancelled_at = Instant::now();
    process.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }));
    let response = process.read();
    assert_eq!(response["id"], 3);
    assert_eq!(response["result"]["stopReason"], "cancelled");
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "cancellation waited for the model process"
    );
    process.finish();
    assert!(
        !data.join("model-completed").exists(),
        "the provider process survived durable cancellation"
    );
}

#[test]
fn redelivering_a_settled_prompt_replays_without_a_second_model_call() {
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
    let turn_id = "c56040df-eb1b-433d-a23b-b2de6dcfd776";

    let first = process.prompt(&session_id, "Idempotent", turn_id);
    let replay = process.prompt(&session_id, "Idempotent", turn_id);

    assert_eq!(first, replay);
    process.finish();
}

#[test]
fn conflicting_frontend_turn_ids_are_rejected_before_admission() {
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
            "prompt": [{ "type": "text", "text": "Idempotent" }],
            "_meta": {
                "requestId": "0c1f7860-62f3-47f7-8f51-9c6ab8a4c9c2",
                "promptId": "84f25320-7fdd-43ee-999a-0211bddcc69a"
            }
        }
    }));
    let response = process.read();
    assert_eq!(response["id"], 3);
    assert_eq!(response["error"]["code"], -32602);
    process.finish();
    assert!(
        !data.join("model-invoked").exists(),
        "ambiguous turn identity reached the model"
    );
}

#[test]
fn a_tool_turn_streams_execution_before_the_final_answer() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(workspace.join("value.txt"), "value\n").expect("write workspace file");
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
            "prompt": [{ "type": "text", "text": "Tool" }],
            "_meta": {
                "requestId": "95a82c47-614d-4b66-a4a5-f3e284d198dc",
                "promptId": "95a82c47-614d-4b66-a4a5-f3e284d198dc"
            }
        }
    }));
    let started = process.read();
    let settled = process.read();
    let answer = process.read();
    let response = process.read();

    assert_eq!(started["params"]["update"]["sessionUpdate"], "tool_call");
    assert_eq!(started["params"]["update"]["toolCallId"], "read-1");
    assert_eq!(started["params"]["update"]["kind"], "read");
    assert_eq!(started["params"]["update"]["status"], "in_progress");
    assert_eq!(
        settled["params"]["update"]["sessionUpdate"],
        "tool_call_update"
    );
    assert_eq!(settled["params"]["update"]["toolCallId"], "read-1");
    assert_eq!(settled["params"]["update"]["status"], "completed");
    assert_eq!(
        settled["params"]["update"]["content"][0]["content"]["text"],
        "value\n"
    );
    assert_eq!(answer["params"]["update"]["content"]["text"], "Read it.");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    process.finish();
}

#[test]
fn version_probe_does_not_start_the_acp_transport() {
    let output = Command::new(env!("CARGO_BIN_EXE_renoa-agent"))
        .arg("--version")
        .output()
        .expect("run version probe");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 version output"),
        format!("renoa-agent {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

fn wait_for_path(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !path.exists() {
        assert!(Instant::now() < deadline, "model process did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
}
