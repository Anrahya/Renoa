mod support;

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    time::{Duration, Instant},
};

use serde_json::json;
use tempfile::tempdir;

use support::{AcpProcess, BRIDGE};

#[test]
fn a_grok_session_exposes_model_and_reasoning_selectors() {
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

    assert_eq!(
        created["result"]["configOptions"],
        json!([
            {
                "id": "model",
                "name": "Model",
                "category": "model",
                "type": "select",
                "currentValue": "grok-test",
                "options": [
                    { "value": "grok-test", "name": "Grok Test" },
                    { "value": "grok-fast", "name": "Grok Fast" }
                ]
            },
            {
                "id": "thought_level",
                "name": "Reasoning",
                "category": "thought_level",
                "type": "select",
                "currentValue": "high",
                "options": [
                    { "value": "low", "name": "Low" },
                    { "value": "medium", "name": "Medium" },
                    { "value": "high", "name": "High" }
                ]
            }
        ])
    );
    process.finish();
}

#[test]
fn changing_reasoning_changes_the_next_provider_request() {
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
    let session_id = created["result"]["sessionId"].as_str().expect("session id");

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/set_config_option",
        "params": {
            "sessionId": session_id,
            "configId": "thought_level",
            "value": "low"
        }
    }));
    let configured = process.read();
    assert_eq!(
        configured["result"]["configOptions"][1]["currentValue"],
        "low"
    );

    let (update, response) = process.prompt(
        session_id,
        "Configured",
        "82cadf3f-9193-4828-a4fb-d5c15d0ab1c9",
    );
    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Reasoning configured."
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");
    process.finish();
}

#[test]
fn a_model_change_survives_session_reload() {
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
    first.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/set_config_option",
        "params": {
            "sessionId": session_id,
            "configId": "thought_level",
            "value": "low"
        }
    }));
    let configured = first.read();
    assert_eq!(
        configured["result"]["configOptions"][1]["currentValue"],
        "low"
    );
    first.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/set_config_option",
        "params": {
            "sessionId": session_id,
            "configId": "model",
            "value": "grok-fast"
        }
    }));
    let configured = first.read();
    assert_eq!(
        configured["result"]["configOptions"][0]["currentValue"],
        "grok-fast"
    );
    first.finish();
    OpenOptions::new()
        .append(true)
        .open(
            data.join("sessions")
                .join(&session_id)
                .join("runtime.jsonl"),
        )
        .expect("open runtime log")
        .write_all(br#"{"provider":"xai","model":"torn""#)
        .expect("append incomplete runtime record");

    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    resumed.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/load",
        "params": {
            "sessionId": session_id,
            "cwd": workspace,
            "mcpServers": []
        }
    }));
    let loaded = resumed.read();
    assert_eq!(
        loaded["result"]["configOptions"][0]["currentValue"],
        "grok-fast"
    );
    assert_eq!(loaded["result"]["configOptions"][1]["currentValue"], "low");
    let (update, response) = resumed.prompt(
        &session_id,
        "Model configured",
        "25559ee8-185f-45ae-adf5-139854fba350",
    );
    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Model configured."
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");
    resumed.finish();
}

#[test]
fn configuration_cannot_change_during_an_active_prompt() {
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
                "requestId": "53e5b56d-926e-4377-b5aa-6ac98d3f003c",
                "promptId": "53e5b56d-926e-4377-b5aa-6ac98d3f003c"
            }
        }
    }));
    wait_for_path(&data.join("model-started"));
    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/set_config_option",
        "params": {
            "sessionId": session_id,
            "configId": "thought_level",
            "value": "low"
        }
    }));
    let rejected = process.read();
    assert_eq!(rejected["id"], 4);
    assert_eq!(
        rejected["error"]["data"],
        "session configuration cannot change during a prompt"
    );

    process.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    }));
    let cancelled = process.read();
    assert_eq!(cancelled["id"], 3);
    assert_eq!(cancelled["result"]["stopReason"], "cancelled");
    process.finish();
    assert!(!data.join("model-completed").exists());
}

fn wait_for_path(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !path.exists() {
        assert!(Instant::now() < deadline, "model process did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
}
