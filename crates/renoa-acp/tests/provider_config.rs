mod support;

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
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
                "currentValue": "xai/grok-test",
                "options": [
                    { "value": "xai/grok-test", "name": "Grok Test (xAI)" },
                    { "value": "xai/grok-fast", "name": "Grok Fast (xAI)" }
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
    let configured = set_option(&mut first, 3, &session_id, "thought_level", "low");
    assert_eq!(
        configured["result"]["configOptions"][1]["currentValue"],
        "low"
    );
    let configured = set_option(&mut first, 4, &session_id, "model", "xai/grok-fast");
    assert_eq!(
        configured["result"]["configOptions"][0]["currentValue"],
        "xai/grok-fast"
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
    let loaded = load_session(&mut resumed, &session_id, &workspace);
    assert_eq!(
        loaded["result"]["configOptions"][0]["currentValue"],
        "xai/grok-fast"
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
    let configured = set_option(&mut resumed, 4, &session_id, "thought_level", "high");
    assert_eq!(
        configured["result"]["configOptions"][1]["currentValue"],
        "high"
    );
    resumed.finish();

    let mut repaired = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    repaired.initialize();
    let loaded = load_session(&mut repaired, &session_id, &workspace);
    assert_eq!(
        loaded["result"]["configOptions"][0]["currentValue"],
        "xai/grok-fast"
    );
    assert_eq!(loaded["result"]["configOptions"][1]["currentValue"], "high");
    repaired.finish();
}

#[test]
fn a_provider_change_survives_reload_and_drives_the_selected_adapter() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");

    let mut first = AcpProcess::spawn_with_providers(
        &workspace,
        &data,
        &bridge,
        &auth_store,
        "xai,opencode-go",
        "xai",
        "grok-test",
    );
    first.initialize();
    let created = first.create_session(&workspace);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    assert_eq!(
        created["result"]["configOptions"][0]["options"],
        json!([
            { "value": "xai/grok-test", "name": "Grok Test (xAI)" },
            { "value": "xai/grok-fast", "name": "Grok Fast (xAI)" },
            {
                "value": "opencode-go/deepseek-test",
                "name": "DeepSeek Test (OpenCode Go)"
            },
            { "value": "opencode-go/grok-test", "name": "Grok Test (OpenCode Go)" }
        ])
    );

    let configured = set_option(
        &mut first,
        3,
        &session_id,
        "model",
        "opencode-go/deepseek-test",
    );
    assert_eq!(
        configured["result"]["configOptions"][0]["currentValue"],
        "opencode-go/deepseek-test"
    );
    let (update, response) = first.prompt(
        &session_id,
        "OpenCode configured",
        "216288ac-7db3-4ab6-a2cb-545eaf562379",
    );
    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "OpenCode configured."
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");
    first.finish();

    let mut resumed = AcpProcess::spawn_with_providers(
        &workspace,
        &data,
        &bridge,
        &auth_store,
        "xai,opencode-go",
        "xai",
        "grok-test",
    );
    resumed.initialize();
    let loaded = load_session(&mut resumed, &session_id, &workspace);
    assert_eq!(
        loaded["result"]["configOptions"][0]["currentValue"],
        "opencode-go/deepseek-test"
    );
    resumed.finish();

    let runtime = fs::read_to_string(data.join("sessions").join(session_id).join("runtime.jsonl"))
        .expect("read runtime selections");
    assert_eq!(
        runtime.lines().last().expect("latest runtime selection"),
        r#"{"provider":"opencode-go","model":"deepseek-test","reasoning":"high"}"#
    );
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
    let cancelled = process
        .read_until_response(3)
        .into_iter()
        .find(|message| message["id"] == 3)
        .expect("prompt response");
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

fn set_option(
    process: &mut AcpProcess,
    id: u64,
    session_id: &str,
    config_id: &str,
    value: &str,
) -> Value {
    process.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/set_config_option",
        "params": {
            "sessionId": session_id,
            "configId": config_id,
            "value": value
        }
    }));
    process.read()
}

fn load_session(process: &mut AcpProcess, session_id: &str, workspace: &std::path::Path) -> Value {
    let (_history, response) = process.load_session(workspace, session_id);
    response
}
