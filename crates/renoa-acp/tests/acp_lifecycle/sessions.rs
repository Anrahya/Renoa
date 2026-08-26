use std::{
    fs,
    time::{Duration, Instant},
};

use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

use super::support::{AcpProcess, BRIDGE};

#[test]
fn compact_is_advertised_after_new_and_load() {
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
    let (updates, created) = first.create_session_with_updates(&workspace);
    assert_eq!(updates.len(), 1);
    assert_compact_command(&updates[0]);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    first.finish();

    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    let (updates, loaded) = resumed.load_session(&workspace, &session_id);
    assert!(loaded["result"].is_object());
    assert_eq!(updates.len(), 1);
    assert_compact_command(&updates[0]);
    resumed.finish();
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
    let messages = process.read_until_response(3);
    let update = messages
        .iter()
        .find(|message| message["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .expect("assistant update");
    let usage = messages
        .iter()
        .find(|message| message["params"]["update"]["sessionUpdate"] == "usage_update")
        .expect("usage update");
    let completed = messages.last().expect("prompt response");
    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Image received."
    );
    assert_eq!(usage["params"]["update"]["used"], 2);
    assert_eq!(usage["params"]["update"]["size"], 500_000);
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
    let first_turn_id = "a19115d8-2796-496a-8763-abe0159efd24";
    let (first_update, first_response) = first.prompt(&session_id, "First", first_turn_id);
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
    assert_eq!(history.len(), 4);
    assert_eq!(
        history[0]["params"]["update"]["sessionUpdate"],
        "user_message_chunk"
    );
    assert_eq!(history[0]["params"]["update"]["content"]["text"], "First");
    assert_eq!(
        history[0]["params"]["update"]["_meta"]["requestId"],
        first_turn_id
    );
    assert_eq!(
        history[1]["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert_eq!(
        history[1]["params"]["update"]["content"]["text"],
        "First response."
    );
    assert_eq!(
        history[2]["params"]["update"],
        json!({
            "sessionUpdate": "usage_update",
            "used": 2,
            "size": 500_000
        })
    );
    assert_compact_command(&history[3]);
    for update in &history[..2] {
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
fn compact_is_silent_idempotent_and_restores_its_durable_usage() {
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
    first.prompt(&session_id, "First", "670cb243-6fe7-4938-af3c-ae4f9a03cfb5");
    let compact_id = "6b506cbb-287f-4fc9-a09a-dbe36eecf7c4";

    first.send_prompt(&session_id, "/compact", compact_id);
    let compact = first.read_until_response(3);
    let used = assert_compaction_messages(&compact);
    first.send_prompt(&session_id, "/compact", compact_id);
    let replayed = first.read_until_response(3);
    assert_eq!(assert_compaction_messages(&replayed), used);
    assert_eq!(
        fs::read_to_string(data.join("compaction-attempts"))
            .expect("read compaction attempt count"),
        "1",
        "stable redelivery sampled the summary model twice"
    );

    first.send_prompt(
        &session_id,
        "/compact now",
        "acda33c5-558a-43c1-a08d-1de92b26be58",
    );
    let rejected = first.read_until_response(3);
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0]["error"]["code"], -32602);
    first.finish();

    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    let (updates, loaded) = resumed.load_session(&workspace, &session_id);
    assert!(loaded["result"].is_object());
    assert_eq!(
        updates
            .iter()
            .filter(|message| {
                message["params"]["update"]["sessionUpdate"] == "user_message_chunk"
            })
            .count(),
        1,
        "the control command leaked into replayable conversation history"
    );
    assert!(
        updates
            .iter()
            .all(|message| { message["params"]["update"]["content"]["text"] != "/compact" })
    );
    let restored_usage = updates
        .iter()
        .find(|message| message["params"]["update"]["sessionUpdate"] == "usage_update")
        .expect("restored usage update");
    assert_eq!(restored_usage["params"]["update"]["used"], used);
    assert_eq!(restored_usage["params"]["update"]["size"], 500_000);
    assert!(updates.iter().any(|message| {
        message["params"]["update"]["sessionUpdate"] == "available_commands_update"
    }));
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
    let messages = process.read_until_response(3);
    let response = messages
        .iter()
        .find(|message| message["id"] == 3)
        .expect("prompt response");
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

fn wait_for_path(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !path.exists() {
        assert!(Instant::now() < deadline, "model process did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn assert_compact_command(message: &serde_json::Value) {
    assert_eq!(
        message["params"]["update"],
        json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{
                "name": "compact",
                "description": "Summarize durable conversation context now"
            }]
        })
    );
}

fn assert_compaction_messages(messages: &[serde_json::Value]) -> u64 {
    assert_eq!(messages.len(), 2, "compaction leaked internal model output");
    assert_eq!(
        messages[0]["params"]["update"]["sessionUpdate"],
        "usage_update"
    );
    assert_eq!(messages[0]["params"]["update"]["size"], 500_000);
    let used = messages[0]["params"]["update"]["used"]
        .as_u64()
        .expect("compacted token estimate");
    assert!(used > 0);
    assert_eq!(messages[1]["result"]["stopReason"], "end_turn");
    used
}
