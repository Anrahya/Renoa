use std::fs;

use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

use super::assertions::assert_equivalent_prompt_outcomes;
use super::support::{AcpProcess, BRIDGE};

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

    assert_equivalent_prompt_outcomes(&first, &replay, turn_id);
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
fn a_discarded_attempt_is_closed_before_the_replayed_response() {
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
        "Truncate",
        "1b0d5f4e-6a2c-4a8f-9d3e-7c5b1a2f8e40",
    );
    let messages = process.read_until_response(3);
    let response = messages.last().expect("prompt response");
    assert_eq!(
        response["result"]["stopReason"], "end_turn",
        "a live unknown outcome must replay and complete the turn: {messages:?}"
    );
    assert_eq!(
        fs::read_to_string(data.join("model-attempts")).expect("model attempts"),
        "2",
        "the unknown outcome must be replayed exactly once"
    );

    let chunks = messages
        .iter()
        .filter(|message| message["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .collect::<Vec<_>>();

    // Protocol version 1 cannot retract the discarded fragment, so the surface
    // closes its message with a discard notice instead of leaving text that
    // reads as a second answer.
    let notice = chunks
        .iter()
        .position(|chunk| {
            chunk["params"]["update"]["_meta"]["renoa.discardedAttempt"] == json!(true)
        })
        .expect("the discarded attempt must be closed with a notice");
    let discarded_id = chunks[notice]["params"]["update"]["messageId"]
        .as_str()
        .expect("discarded message id");
    assert_eq!(
        chunks
            .iter()
            .filter(|chunk| chunk["params"]["update"]["messageId"].as_str() == Some(discarded_id))
            .map(|chunk| {
                chunk["params"]["update"]["content"]["text"]
                    .as_str()
                    .expect("discarded text")
            })
            .collect::<String>(),
        "stale partial \n\n[Renoa discarded this partial response and retried the request.]",
        "the notice must close the message that carried the discarded fragment: {messages:?}"
    );

    let answer = chunks.iter().skip(notice + 1).collect::<Vec<_>>();
    let answer_ids = answer
        .iter()
        .map(|chunk| {
            chunk["params"]["update"]["messageId"]
                .as_str()
                .expect("answer message id")
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        answer_ids.len(),
        1,
        "the replay must arrive as one assistant response after the notice: {messages:?}"
    );
    assert!(
        !answer_ids.contains(discarded_id),
        "the replayed response must not continue the discarded message: {messages:?}"
    );
    assert_eq!(
        answer
            .iter()
            .map(|chunk| {
                chunk["params"]["update"]["content"]["text"]
                    .as_str()
                    .expect("answer text")
            })
            .collect::<String>(),
        "fresh complete",
        "the frontend must receive the replayed response alone: {messages:?}"
    );
    process.finish();
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
    let messages = process.read_until_response(3);
    let updates = &messages[..messages.len() - 1];
    assert_eq!(
        updates
            .iter()
            .map(|message| &message["params"]["update"]["sessionUpdate"])
            .collect::<Vec<_>>(),
        [
            "agent_message_chunk",
            "usage_update",
            "tool_call",
            "tool_call_update",
            "usage_update",
            "agent_message_chunk",
        ]
    );
    let before_tool = &updates[0];
    let started = &updates[2];
    let settled = &updates[3];
    let answer = &updates[5];
    let response = messages.last().expect("prompt response");

    assert_eq!(
        before_tool["params"]["update"]["content"]["text"],
        "Checking. "
    );
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
    for usage in [&updates[1], &updates[4]] {
        assert_eq!(usage["params"]["update"]["used"], 2);
        assert_eq!(usage["params"]["update"]["size"], 500_000);
    }
    let before_tool_id = before_tool["params"]["update"]["messageId"]
        .as_str()
        .expect("pre-tool assistant message id");
    let answer_id = answer["params"]["update"]["messageId"]
        .as_str()
        .expect("final assistant message id");
    Uuid::parse_str(before_tool_id).expect("pre-tool message id is a UUID");
    Uuid::parse_str(answer_id).expect("final message id is a UUID");
    assert_ne!(
        before_tool_id, answer_id,
        "separate assistant messages must have separate ACP identities"
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");
    process.finish();
}
