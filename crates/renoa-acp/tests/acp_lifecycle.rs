#[path = "support/assertions.rs"]
mod assertions;
#[path = "acp_lifecycle/close.rs"]
mod close;
#[path = "acp_lifecycle/delete.rs"]
mod delete;
#[path = "acp_lifecycle/observability.rs"]
mod observability;
#[path = "acp_lifecycle/sessions.rs"]
mod sessions;
mod support;
#[path = "acp_lifecycle/turns.rs"]
mod turns;

use std::{fs, process::Command};

use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

use renoa_kernel::{EffectStatus, Kernel, OperationStatus, SessionId};
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
    assert!(
        initialized["result"]["agentCapabilities"]["sessionCapabilities"]["delete"].is_object()
    );

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
        update["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert_eq!(
        update["params"]["update"]["content"],
        json!({ "type": "text", "text": "Hello back." })
    );
    let message_id = update["params"]["update"]["messageId"]
        .as_str()
        .expect("assistant message id");
    Uuid::parse_str(message_id).expect("assistant message id is a UUID");
    assert_ne!(message_id, turn_id);
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
fn a_provider_failure_is_a_terminal_jsonrpc_error() {
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

    process.send_prompt(
        &session_id,
        "FailProvider",
        "3d5a2c1e-8b47-4f9a-9c1d-0e2f3a4b5c6d",
    );
    let messages = process.read_until_response(3);
    let response = messages
        .last()
        .expect("provider failure must produce a JSON-RPC response");
    assert!(
        response.get("error").is_some(),
        "provider failure must not leave the ACP prompt working: {messages:?}"
    );
    assert!(response.get("result").is_none());
    let data = response["error"]["data"]
        .as_str()
        .expect("JSON-RPC error data");
    assert!(
        data.contains("connection reset before an HTTP response (ECONNRESET)"),
        "ACP error must surface the provider failure: {data}"
    );
    assert!(
        !data.contains("effect outcome is unknown"),
        "known pre-inference failure must not be abandoned: {data}"
    );
    process.finish();
}

#[test]
fn unknown_provider_outcome_keeps_detailed_ui_error_and_unknown_effect() {
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

    process.send_prompt(
        &session_id,
        "FailAfterDispatch",
        "7c1e9b2a-4d83-41f0-9a66-2b8c0d1e4f70",
    );
    let messages = process.read_until_response(3);
    let response = messages
        .last()
        .expect("unknown provider outcome must produce a JSON-RPC response");
    assert!(
        response.get("error").is_some(),
        "unknown provider outcome must not leave the ACP prompt working: {messages:?}"
    );
    let data_text = response["error"]["data"]
        .as_str()
        .expect("JSON-RPC error data");
    assert!(
        data_text.contains("connection reset after the request may have been transmitted"),
        "ACP error must surface the provider failure: {data_text}"
    );
    assert!(
        !data_text.contains("effect outcome is unknown"),
        "JSON-RPC must not replace the provider error with the abandoned-operation reason: {data_text}"
    );
    let info = messages.iter().find(|message| {
        message["method"] == "session/update"
            && message["params"]["update"]["sessionUpdate"] == "session_info_update"
    });
    let info = info.expect("ACP must emit a redacted ModelRequestFailed session update");
    assert_eq!(
        info["params"]["update"]["_meta"]["renoa.modelRequestFailed"]["outcome_unknown"],
        true
    );
    assert!(
        info["params"]["update"]["_meta"]["renoa.modelRequestFailed"]["message"]
            .as_str()
            .is_some_and(|message| message
                .contains("connection reset after the request may have been transmitted")),
        "session update must carry the concise provider error: {info}"
    );
    assert_eq!(
        info["params"]["update"]["_meta"]["renoa.modelRequestFailed"]["diagnostic"]["provider_message"],
        "The upstream closed the connection after reading the chat completion request."
    );

    process.finish();
    let snapshot = Kernel::open(
        data.join("sessions")
            .join(&session_id)
            .join("kernel.sqlite3"),
    )
    .expect("open kernel")
    .inspect(SessionId::from_uuid(
        Uuid::parse_str(&session_id).expect("session UUID"),
    ))
    .expect("inspect abandoned unknown effect");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(snapshot.operations[0].effects[0].outcome, None);
}

#[test]
fn a_later_model_failure_does_not_reuse_a_stale_context_rejection() {
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

    process.send_prompt(
        &session_id,
        "FailContext",
        "11111111-1111-4111-8111-111111111111",
    );
    let first = process.read_until_response(3);
    let first_error = first
        .last()
        .expect("context rejection must produce a JSON-RPC response")["error"]["data"]
        .as_str()
        .expect("first JSON-RPC error data");
    assert!(
        first_error.contains("context window"),
        "first prompt must surface the context rejection: {first_error}"
    );

    process.send_prompt_id(
        4,
        &session_id,
        "FailAfterDispatch",
        "22222222-2222-4222-8222-222222222222",
    );
    let second = process.read_until_response(4);
    let second_error = second
        .last()
        .expect("later failure must produce a JSON-RPC response")["error"]["data"]
        .as_str()
        .expect("second JSON-RPC error data");
    assert!(
        second_error.contains("connection reset after the request may have been transmitted"),
        "stale context rejection must not replace a later model failure: {second_error}"
    );
    assert!(
        !second_error.contains("context window"),
        "later JSON-RPC must not keep the first prompt's context error: {second_error}"
    );
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

#[test]
fn model_catalog_probe_is_read_only_and_marks_runtime_defaults() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    let data = directory.path().join("must-not-exist");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");

    let output = Command::new(env!("CARGO_BIN_EXE_renoa-agent"))
        .args(["models", "--json"])
        .env("RENOA_DATA_DIR", &data)
        .env("RENOA_MODEL_BRIDGE", &bridge)
        .env("RENOA_MODEL_PROVIDERS", "xai,opencode-go")
        .env("RENOA_MODEL_PROVIDER", "opencode-go")
        .env("RENOA_MODEL", "deepseek-test")
        .env("RENOA_MODEL_AUTH_STORE", &auth_store)
        .output()
        .expect("run model catalog probe");

    assert!(
        output.status.success(),
        "catalog probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(
        !data.exists(),
        "catalog probing created durable session state"
    );
    let catalog: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("decode model catalog");
    assert_eq!(
        catalog,
        json!({
            "models": [
                {
                    "id": "xai/grok-test",
                    "name": "Grok Test (xAI)",
                    "isDefault": false,
                    "reasoningLevels": [
                        { "id": "low", "name": "Low" },
                        { "id": "medium", "name": "Medium" },
                        { "id": "high", "name": "High" }
                    ],
                    "defaultReasoning": "high"
                },
                {
                    "id": "xai/grok-fast",
                    "name": "Grok Fast (xAI)",
                    "isDefault": false,
                    "reasoningLevels": [
                        { "id": "off", "name": "Off" },
                        { "id": "low", "name": "Low" },
                        { "id": "high", "name": "High" }
                    ],
                    "defaultReasoning": "high"
                },
                {
                    "id": "opencode-go/deepseek-test",
                    "name": "DeepSeek Test (OpenCode Go)",
                    "isDefault": true,
                    "reasoningLevels": [
                        { "id": "low", "name": "Low" },
                        { "id": "high", "name": "High" }
                    ],
                    "defaultReasoning": "high"
                },
                {
                    "id": "opencode-go/grok-test",
                    "name": "Grok Test (OpenCode Go)",
                    "isDefault": false,
                    "reasoningLevels": [
                        { "id": "off", "name": "Off" },
                        { "id": "high", "name": "High" }
                    ],
                    "defaultReasoning": "high"
                }
            ]
        })
    );
}
