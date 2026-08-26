use std::{
    fs,
    time::{Duration, Instant},
};

use serde_json::json;
use tempfile::tempdir;

use super::support::{AcpProcess, BRIDGE};

#[test]
fn closing_a_session_releases_the_process_for_another_session() {
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
    let first_session = created["result"]["sessionId"].as_str().expect("session id");

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/close",
        "params": { "sessionId": first_session }
    }));
    let closed = process.read();
    assert_eq!(closed["id"], 3);
    assert!(closed["result"].is_object());

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/new",
        "params": { "cwd": workspace, "mcpServers": [] }
    }));
    let recreated = process
        .read_until_response(4)
        .pop()
        .expect("second session response");
    assert_eq!(recreated["id"], 4);
    assert_ne!(recreated["result"]["sessionId"], first_session);
    process.finish();
}

#[test]
fn closing_an_active_session_waits_until_provider_work_is_stopped() {
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
                "requestId": "ccb13931-0719-4f97-bb64-ea8080bf84e0",
                "promptId": "ccb13931-0719-4f97-bb64-ea8080bf84e0"
            }
        }
    }));
    wait_for_path(&data.join("model-started"));
    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/close",
        "params": { "sessionId": session_id }
    }));

    let mut prompt = None;
    let mut close = None;
    while prompt.is_none() || close.is_none() {
        let message = process.read();
        if message["id"] == 3 {
            prompt = Some(message);
        } else if message["id"] == 4 {
            close = Some(message);
        }
    }
    let prompt = prompt.expect("prompt response");
    let close = close.expect("close response");
    assert_eq!(prompt["result"]["stopReason"], "cancelled");
    assert!(close["result"].is_object());
    assert!(
        !data.join("model-completed").exists(),
        "session/close returned while provider work was still live"
    );

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "session/new",
        "params": { "cwd": workspace, "mcpServers": [] }
    }));
    let recreated = process
        .read_until_response(5)
        .pop()
        .expect("second session response");
    assert_eq!(recreated["id"], 5);
    assert!(recreated["result"]["sessionId"].is_string());
    process.finish();
}

fn wait_for_path(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !path.exists() {
        assert!(Instant::now() < deadline, "model process did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
}
