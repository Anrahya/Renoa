use std::{fs, path::Path};

use renoa_kernel::SessionId;
use renoa_local::LocalSession;
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

use super::support::{AcpProcess, BRIDGE};

#[derive(Clone, Copy)]
enum Failure {
    Provider,
    Model,
    Bridge,
    TraceCorrupt,
    TraceUnavailable,
    KernelCorrupt,
}

#[test]
fn history_survives_a_disabled_provider() {
    inspect_with_failure(Failure::Provider);
}
#[test]
fn history_survives_a_missing_model() {
    inspect_with_failure(Failure::Model);
}
#[test]
fn history_survives_a_missing_model_bridge() {
    inspect_with_failure(Failure::Bridge);
}
#[test]
fn history_survives_a_corrupt_trace_store() {
    inspect_with_failure(Failure::TraceCorrupt);
}
#[test]
fn history_survives_an_unavailable_trace_store() {
    inspect_with_failure(Failure::TraceUnavailable);
}
#[test]
fn corrupt_authoritative_history_is_not_hidden_by_inspection() {
    inspect_with_failure(Failure::KernelCorrupt);
}

fn inspect_with_failure(failure: Failure) {
    let fixture = settled_session();
    let SettledSession {
        workspace,
        data,
        bridge,
        auth,
        id,
        session_id,
        kernel,
        expected,
        trace,
        ..
    } = &fixture;
    break_dependency(failure, bridge, trace, kernel);
    let mut resumed = if matches!(failure, Failure::Provider) {
        AcpProcess::spawn_with_providers(
            workspace,
            data,
            bridge,
            auth,
            "opencode-go",
            "opencode-go",
            "deepseek-test",
        )
    } else {
        AcpProcess::spawn(workspace, data, bridge, auth)
    };
    resumed.initialize();
    let (history, loaded) = resumed.load_session(workspace, id);
    if matches!(failure, Failure::KernelCorrupt) {
        assert!(loaded["error"].is_object(), "{loaded}");
        assert!(history.is_empty());
        resumed.finish();
        return;
    }
    assert!(
        loaded["result"].is_object(),
        "history should load: {loaded}"
    );
    let unavailable = loaded["result"]["_meta"]["renoa.executionUnavailable"]
        .as_str()
        .expect("explicit execution status");
    assert!(!unavailable.is_empty());
    let diagnostic = &loaded["result"]["_meta"]["renoa.traceUnavailable"];
    assert_eq!(
        diagnostic.is_string(),
        matches!(failure, Failure::TraceCorrupt | Failure::TraceUnavailable)
    );
    assert_eq!(
        history.len(),
        expected.len(),
        "no invented configuration, usage, or commands"
    );
    for (event, entry) in history.iter().zip(expected) {
        assert_eq!(event["params"]["update"]["messageId"], entry.event_id);
    }
    assert_eq!(
        history[0]["params"]["update"]["_meta"]["requestId"],
        expected[0].command_id.to_string()
    );

    assert_eq!(history[0]["params"]["update"]["content"]["text"], "First");
    assert_eq!(
        history[1]["params"]["update"]["content"]["text"],
        "First response."
    );
    assert!(
        LocalSession::load(kernel, *session_id).is_err(),
        "inspection must keep exclusive kernel ownership"
    );
    resumed.send_prompt(id, "Second", "22222222-2222-4222-8222-222222222222");
    let response = resumed
        .read_until_response(3)
        .pop()
        .expect("prompt rejection");
    assert!(
        response["error"]["data"]
            .as_str()
            .expect("execution error")
            .contains("unavailable")
    );
    resumed.send(
        &json!({"jsonrpc": "2.0", "id": 4, "method": "session/close", "params": {"sessionId": id}}),
    );
    assert!(resumed.read()["result"].is_object());
    resumed.finish();
    let stored =
        LocalSession::load(kernel, *session_id).expect("closed inspection releases ownership");
    assert_eq!(
        stored.history().expect("history after rejected execution"),
        *expected
    );
}

fn break_dependency(failure: Failure, bridge: &Path, trace: &Path, kernel: &Path) {
    match failure {
        Failure::Provider => {}
        Failure::Model => fs::write(
            bridge,
            BRIDGE.replace("id: \"grok-test\"", "id: \"removed-test\""),
        )
        .expect("remove selected model"),
        Failure::Bridge => fs::remove_file(bridge).expect("remove model adapter"),
        Failure::TraceCorrupt => {
            fs::write(trace, "invalid diagnostic database").expect("corrupt trace");
        }
        Failure::TraceUnavailable => {
            fs::remove_file(trace).expect("remove trace");
            fs::create_dir(trace).expect("make trace unavailable");
        }
        Failure::KernelCorrupt => {
            fs::write(kernel, "invalid authoritative database").expect("corrupt kernel");
        }
    }
}

struct SettledSession {
    _directory: tempfile::TempDir,
    workspace: std::path::PathBuf,
    data: std::path::PathBuf,
    bridge: std::path::PathBuf,
    auth: std::path::PathBuf,
    id: String,
    session_id: SessionId,
    kernel: std::path::PathBuf,
    expected: Vec<renoa_local::LocalHistoryEntry>,
    trace: std::path::PathBuf,
}

fn settled_session() -> SettledSession {
    let directory = tempdir().expect("fixture directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("workspace");
    fs::write(&bridge, BRIDGE).expect("fixture model bridge");
    fs::write(&auth, "").expect("fixture credentials");
    let mut first = AcpProcess::spawn(&workspace, &data, &bridge, &auth);
    first.initialize();
    let created = first.create_session(&workspace);
    let id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    assert_eq!(
        first
            .prompt(&id, "First", "a19115d8-2796-496a-8763-abe0159efd24")
            .1["result"]["stopReason"],
        "end_turn"
    );
    first.finish();
    let session_id = SessionId::from_uuid(Uuid::parse_str(&id).expect("UUID"));
    let root = data.join("sessions").join(&id);
    let kernel = root.join("kernel.sqlite3");
    let stored = LocalSession::load(&kernel, session_id).expect("settled session");
    let expected = stored.history().expect("intact history");
    drop(stored);
    let trace = root.join("trace.sqlite3");
    SettledSession {
        _directory: directory,
        workspace,
        data,
        bridge,
        auth,
        id,
        session_id,
        kernel,
        expected,
        trace,
    }
}
