use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use renoa_kernel::{Kernel, OperationStatus, SessionId};
use rusqlite::Connection;
use tempfile::tempdir;
use uuid::Uuid;

use super::support::{AcpProcess, BRIDGE};

#[test]
fn a_settled_turn_survives_a_lost_acp_response_without_another_model_call() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let turn_id = "bd13fd21-4b84-4f15-b043-8e8eb8889fea";

    let mut first = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    first.initialize();
    let created = first.create_session(&workspace);
    let session = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    first.send_prompt(&session, "Idempotent", turn_id);
    wait_for_settled_operation(&data, &session);
    first.kill();

    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    let (_history, loaded) = resumed.load_session(&workspace, &session);
    assert!(loaded.get("result").is_some(), "load failed: {loaded}");
    let (update, response) = resumed.prompt(&session, "Idempotent", turn_id);

    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Exactly once."
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");
    resumed.finish();
    assert_single_operation(&data, &session, OperationStatus::Completed);
}

#[test]
fn an_interrupted_safe_model_call_replays_under_the_same_kernel_turn() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let turn_id = "4ada1bd3-50b3-46cd-ac49-d08e7d400e17";

    let mut first = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    first.initialize();
    let created = first.create_session(&workspace);
    let session = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    first.send_prompt(&session, "Crash model", turn_id);
    wait_for_path(&data.join("model-started"), "model bridge dispatch");
    first.kill();
    kill_process_group(&data.join("model-child-pid"));

    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    let (_history, loaded) = resumed.load_session(&workspace, &session);
    assert!(loaded.get("result").is_some(), "load failed: {loaded}");
    let (update, response) = resumed.prompt(&session, "Crash model", turn_id);

    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Recovered model call."
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        fs::read_to_string(data.join("model-attempts")).expect("read model attempt count"),
        "2",
        "a dispatched safe model effect should replay exactly once after process loss"
    );
    resumed.finish();
    assert_single_operation(&data, &session, OperationStatus::Completed);
}

#[test]
fn an_interrupted_bash_call_is_closed_honestly_without_reexecution() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let turn_id = "42864b69-c67d-42ef-91bf-8b0850fe0a23";

    let mut first = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    first.initialize();
    let created = first.create_session(&workspace);
    let session = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    first.send_prompt(&session, "Crash bash", turn_id);
    wait_for_path(&data.join("unsafe-started"), "Bash dispatch");
    first.kill();
    kill_process_group(&data.join("unsafe-child-pid"));

    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    let (_history, loaded) = resumed.load_session(&workspace, &session);
    assert!(loaded.get("result").is_some(), "load failed: {loaded}");
    resumed.send_prompt(&session, "Crash bash", turn_id);
    let retry = resumed.read_until_response(3);
    let response = retry.last().expect("recovery response");

    assert_eq!(response["error"]["code"], -32603);
    assert_eq!(
        response["error"]["data"],
        "Renoa operation failed: effect outcome is unknown; operation was abandoned without replay"
    );
    assert_eq!(
        fs::read_to_string(data.join("unsafe-started"))
            .expect("read unsafe dispatch marker")
            .lines()
            .count(),
        1,
        "the unknown Bash effect was dispatched again"
    );
    assert!(
        !data.join("unsafe-completed").exists(),
        "the killed Bash effect completed after recovery"
    );
    resumed.finish();
    assert_single_operation(&data, &session, OperationStatus::Failed);

    let mut observer = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    observer.initialize();
    let (history, loaded) = observer.load_session(&workspace, &session);
    assert!(loaded.get("result").is_some(), "load failed: {loaded}");
    let unknown = history
        .iter()
        .find(|message| {
            message["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && message["params"]["update"]["toolCallId"] == "unsafe-bash-1"
        })
        .expect("replayed unknown Bash result");
    assert_eq!(unknown["params"]["update"]["status"], "failed");
    assert_eq!(
        unknown["params"]["update"]["content"][0]["content"]["text"],
        "This tool may have finished, but Renoa could not recover its result. It was not run again."
    );
    observer.finish();
}

fn wait_for_path(path: &Path, description: &str) {
    wait_until(description, || path.exists());
}

fn wait_for_settled_operation(data: &Path, session: &str) {
    let database = data.join("sessions").join(session).join("kernel.sqlite3");
    wait_until("durable operation settlement", || {
        let Ok(connection) = Connection::open(&database) else {
            return false;
        };
        connection
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE outcome_json IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .is_ok_and(|count| count == 1)
    });
}

fn assert_single_operation(data: &Path, session: &str, status: OperationStatus) {
    let session_id = SessionId::from_uuid(Uuid::parse_str(session).expect("session UUID"));
    let snapshot = Kernel::open(data.join("sessions").join(session).join("kernel.sqlite3"))
        .expect("open kernel")
        .inspect(session_id)
        .expect("inspect session");
    assert_eq!(snapshot.operations.len(), 1, "duplicate kernel turn");
    assert_eq!(snapshot.operations[0].status, status);
}

fn kill_process_group(pid_path: &Path) {
    let pid = fs::read_to_string(pid_path)
        .expect("read child process id")
        .trim()
        .parse::<i32>()
        .expect("child process id is an integer");
    match killpg(Pid::from_raw(pid), Signal::SIGKILL) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(error) => panic!("kill child process group: {error}"),
    }
    wait_until("killed child process group to exit", || {
        match killpg(Pid::from_raw(pid), None) {
            Err(nix::errno::Errno::ESRCH) => true,
            Ok(()) => false,
            Err(error) => panic!("inspect killed child process group: {error}"),
        }
    });
}

fn wait_until(description: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
