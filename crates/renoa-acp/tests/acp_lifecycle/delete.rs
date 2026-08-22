use std::fs;

use serde_json::json;
use tempfile::tempdir;

use super::support::{AcpProcess, BRIDGE};

#[test]
fn a_closed_session_can_be_deleted_idempotently() {
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
    assert!(
        initialized["result"]["agentCapabilities"]["sessionCapabilities"]["delete"].is_object()
    );
    let created = process.create_session(&workspace);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let session_directory = data.join("sessions").join(&session_id);
    assert!(session_directory.is_dir());

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/delete",
        "params": { "sessionId": session_id.to_uppercase() }
    }));
    let active_delete = process.read();
    assert_eq!(active_delete["id"], 3);
    assert_eq!(active_delete["error"]["code"], -32602);
    assert!(
        active_delete["error"]["data"]
            .as_str()
            .expect("actionable deletion error")
            .contains("close")
    );
    assert!(session_directory.is_dir());

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/close",
        "params": { "sessionId": session_id }
    }));
    assert!(process.read()["result"].is_object());

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "session/delete",
        "params": { "sessionId": session_id }
    }));
    let deleted = process.read();
    assert_eq!(deleted["id"], 5);
    assert!(deleted["result"].is_object());
    assert!(!session_directory.exists());

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "session/delete",
        "params": { "sessionId": session_id }
    }));
    let repeated = process.read();
    assert_eq!(repeated["id"], 6);
    assert!(repeated["result"].is_object());

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "session/load",
        "params": {
            "sessionId": session_id,
            "cwd": workspace,
            "mcpServers": []
        }
    }));
    let missing = process.read();
    assert_eq!(missing["id"], 7);
    assert!(missing["error"].is_object());
    assert!(!session_directory.exists());

    process.finish();
}
