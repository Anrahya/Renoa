mod support;

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::Path,
};

use renoa_kernel::{Command, CommandId, Kernel, SessionId};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

use support::{AcpProcess, BRIDGE};

#[test]
fn pre_kernel_session_storage_is_rejected_explicitly() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    let session_id = "62d68ada-4f4f-4c2e-ad3e-e54af67e87d2";
    fs::create_dir(&workspace).expect("create workspace");
    fs::create_dir_all(data.join("sessions").join(session_id))
        .expect("create legacy session directory");
    fs::write(
        data.join("sessions").join(session_id).join("session.json"),
        serde_json::to_vec(&json!({
            "version": 1,
            "session_id": session_id,
            "workspace": workspace
        }))
        .expect("encode legacy manifest"),
    )
    .expect("write legacy manifest");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let mut process = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    process.initialize();

    let (_history, rejected) = process.load_session(&workspace, session_id);

    assert_eq!(rejected["error"]["code"], -32602);
    assert_eq!(
        rejected["error"]["data"],
        "session storage version 1 is unsupported; expected 3"
    );
    process.finish();
}

#[test]
fn settled_prompt_redelivery_survives_an_acp_process_restart() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let turn_id = "d336af88-785c-4fc1-8837-a94b54a9c77a";

    let mut first = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    first.initialize();
    let created = first.create_session(&workspace);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let initial = first.prompt(&session_id, "Idempotent", turn_id);
    first.finish();

    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    let (_history, loaded) = resumed.load_session(&workspace, &session_id);
    assert!(loaded.get("result").is_some(), "load failed: {loaded}");
    let replay = resumed.prompt(&session_id, "Idempotent", turn_id);

    assert_eq!(initial, replay);
    resumed.finish();
}

#[test]
fn reusing_a_turn_identity_with_different_content_fails_before_the_model() {
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
    let turn_id = "2c90a02a-530a-4a0b-8605-8c9f280c195c";
    process.prompt(&session_id, "Idempotent", turn_id);

    process.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [
                { "type": "text", "text": "Idempotent" },
                { "type": "image", "data": "AAEC", "mimeType": "image/png" }
            ],
            "_meta": { "requestId": turn_id, "promptId": turn_id }
        }
    }));
    let rejected = process.read();

    assert_eq!(rejected["id"], 4);
    assert_eq!(rejected["error"]["code"], -32602);
    assert!(
        rejected["error"]["data"]
            .as_str()
            .expect("error data")
            .contains("with different content")
    );
    process.finish();
}

#[test]
fn an_unfinished_turn_requires_its_stable_identity_before_execution() {
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
    let session = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    first.finish();

    let session_id = queue_unfinished_turn(&data, &session);

    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    let (_history, loaded) = resumed.load_session(&workspace, &session);
    assert!(loaded.get("result").is_some(), "load failed: {loaded}");
    resumed.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session,
            "prompt": [{ "type": "text", "text": "Idempotent" }],
            "_meta": {
                "requestId": "a87dbd57-0e76-44da-b28e-649871398228",
                "promptId": "a87dbd57-0e76-44da-b28e-649871398228"
            }
        }
    }));
    let rejected = resumed.read();

    assert_eq!(rejected["error"]["code"], -32602);
    assert!(
        rejected["error"]["data"]
            .as_str()
            .expect("error data")
            .contains("unfinished operation"),
        "unexpected response: {rejected}"
    );
    assert!(
        !data.join("model-invoked").exists(),
        "the older queued operation reached the model"
    );

    resumed.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/prompt",
        "params": {
            "sessionId": session,
            "prompt": [{ "type": "text", "text": "Idempotent" }],
            "_meta": {
                "requestId": "01b77854-6b6c-4b77-a86f-219de44bed66",
                "promptId": "01b77854-6b6c-4b77-a86f-219de44bed66"
            }
        }
    }));
    let update = resumed.read();
    let completed = resumed.read();
    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Exactly once."
    );
    assert_eq!(completed["result"]["stopReason"], "end_turn");
    resumed.finish();
    assert!(
        data.join("model-invoked").exists(),
        "the stable retry did not resume the queued operation"
    );
    assert_eq!(
        Kernel::open(data.join("sessions").join(&session).join("kernel.sqlite3"))
            .expect("reopen kernel")
            .inspect(session_id)
            .expect("inspect session")
            .operations
            .len(),
        1,
        "the rejected turn was admitted behind unfinished work"
    );
}

#[test]
fn a_competing_process_cannot_repair_storage_before_it_owns_the_session() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");

    let mut owner = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    owner.initialize();
    let created = owner.create_session(&workspace);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let selection = data
        .join("sessions")
        .join(&session_id)
        .join("runtime.jsonl");
    let torn = br#"{"provider":"xai","model":"torn""#;
    OpenOptions::new()
        .append(true)
        .open(&selection)
        .expect("open runtime log")
        .write_all(torn)
        .expect("append crash tail");

    let mut competing = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    competing.initialize();
    let (_history, rejected) = competing.load_session(&workspace, &session_id);

    assert_eq!(rejected["error"]["code"], -32603);
    assert!(
        fs::read(&selection)
            .expect("read runtime log")
            .ends_with(torn),
        "a process without the kernel lease modified Host storage"
    );
    competing.finish();
    owner.finish();
}

fn queue_unfinished_turn(data: &Path, session: &str) -> SessionId {
    let session_id = SessionId::from_uuid(Uuid::parse_str(session).expect("session UUID"));
    let kernel = Kernel::open(data.join("sessions").join(session).join("kernel.sqlite3"))
        .expect("open kernel");
    kernel
        .submit(
            session_id,
            Command::new(
                CommandId::from_uuid(
                    Uuid::parse_str("01b77854-6b6c-4b77-a86f-219de44bed66")
                        .expect("queued command UUID"),
                ),
                json!({ "content": [{ "type": "text", "text": "Idempotent" }] }),
            ),
        )
        .expect("queue interrupted turn");
    session_id
}
