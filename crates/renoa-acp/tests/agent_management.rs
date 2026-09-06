use std::{
    path::Path,
    process::{Command, Output},
};

use serde_json::{Value, json};
use tempfile::tempdir;

fn run(data: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_renoa-agent"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("test PATH"))
        .env("RENOA_DATA_DIR", data)
        .env("RENOA_MODEL_BRIDGE", data.join("model.mjs"))
        .env("RENOA_MODEL_PROVIDER", "xai")
        .env("RENOA_MODEL", "grok-test")
        .env("RENOA_MODEL_AUTH_STORE", data.join("auth.sqlite"))
        .arg("agents")
        .args(arguments)
        .output()
        .expect("management command")
}

fn result(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("management JSON")
}

#[test]
fn separate_cli_processes_manage_one_durable_roster_without_execution_dependencies() {
    let directory = tempdir().expect("Host directory");
    let data = directory.path();
    let parent = "66b9dba1-a904-47c8-82a2-a6861d1bf7bc";
    let child = "70e49c48-f3c6-48c8-be78-5b7597618a2e";
    let before = result(&run(data, &["list"]));
    let record = result(&run(data, &["ensure", parent, "Operator"]));
    assert_eq!(record["id"], parent);
    assert_eq!(result(&run(data, &["ensure", parent, "Operator"])), record);
    assert!(!run(data, &["ensure", parent, "Different"]).status.success());
    let specialist = result(&run(data, &["ensure", child, "News", parent]));
    assert_eq!(specialist["created_by"], parent);
    assert_eq!(result(&run(data, &["show", child])), specialist);
    let after = result(&run(data, &["list"]));
    assert_eq!(before["host_id"], after["host_id"]);
    assert_eq!(after["agents"].as_array().expect("roster").len(), 2);
    assert_eq!(specialist["profile"], json!(renoa_local::ALPHA_PROFILE_ID));
    assert!(!data.join("model.mjs").exists());
}

#[test]
fn cli_session_creation_retries_reopen_the_same_agent_and_session() {
    let directory = tempdir().expect("Host directory");
    let data = directory.path();
    std::fs::write(data.join("model.mjs"), include_str!("support/bridge.js"))
        .expect("model fixture");
    std::fs::write(data.join("auth.sqlite"), "").expect("auth fixture");
    let workspace = data.join("workspace");
    std::fs::create_dir(&workspace).expect("workspace");
    let workspace = workspace.to_str().expect("workspace path");
    let agent = "92a24461-47f1-4e6c-a709-5fb4f03a7d10";
    let session = "3c8b3b62-ad4d-4411-ac20-f3299742fb9e";
    result(&run(data, &["ensure", agent, "Operator"]));
    let created = result(&run(data, &["session", agent, session, workspace]));
    assert_eq!(created, json!({"agent_id": agent, "session_id": session}));
    assert_eq!(
        result(&run(data, &["session", agent, session, workspace])),
        created
    );
}
