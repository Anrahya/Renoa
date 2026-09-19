use std::{
    path::Path,
    process::{Command, Output},
};

use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalHost,
    LocalHostAdapters, LocalModelConfiguration, ModelProvider,
};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const PRESET_ID: &str = "renoa.coding.alpha.v1";

fn run(data: &Path, configured_agent: &str, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_renoa-agent"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("test PATH"))
        .env("RENOA_DATA_DIR", data)
        .env("RENOA_AGENT_ID", configured_agent)
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

/// Provisions one canonical agent through the real Host path.
fn provision(data: &Path, name: &str) -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("provisioning runtime");
    let agent_id = runtime.block_on(async {
        let host = LocalHost::new(
            data,
            LocalModelConfiguration::new(
                data.join("model.mjs"),
                vec![ModelProvider::Xai],
                ModelProvider::Xai,
                "grok-test",
                data.join("auth.sqlite"),
            ),
            Vec::new(),
            LocalHostAdapters::default(),
        )
        .expect("provisioning Host");
        host.create_agent(
            AgentCreator::System {
                component: "management-test".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(PRESET_ID).expect("alpha preset id"),
                name,
            ),
            CancellationToken::new(),
        )
        .await
        .expect("provision agent")
        .id
    });
    agent_id.to_string()
}

#[test]
fn separate_cli_processes_read_one_durable_roster_without_execution_dependencies() {
    let directory = tempdir().expect("Host directory");
    let data = directory.path();
    let operator = provision(data, "Operator");
    let news = provision(data, "News");
    let listed = result(&run(data, &operator, &["list"]));
    assert_eq!(listed["agents"].as_array().expect("roster").len(), 2);
    assert_eq!(
        listed["host_id"],
        result(&run(data, &operator, &["list"]))["host_id"]
    );
    let shown = result(&run(data, &operator, &["show", &news]));
    assert_eq!(shown["id"], json!(news));
    assert_eq!(shown["name"], json!("News"));
    assert_eq!(shown["preset_id"], json!(PRESET_ID));
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
    let agent = provision(data, "Operator");
    let session = "3c8b3b62-ad4d-4411-ac20-f3299742fb9e";
    let created = result(&run(data, &agent, &["session", &agent, session, workspace]));
    assert_eq!(created, json!({"agent_id": agent, "session_id": session}));
    assert_eq!(
        result(&run(data, &agent, &["session", &agent, session, workspace])),
        created
    );
}
