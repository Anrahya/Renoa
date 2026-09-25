#[allow(
    dead_code,
    reason = "the shared support module is compiled whole; this target spawns only pre-provisioned agents"
)]
mod support;

use std::{fs, path::Path};

use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalHost,
    LocalHostAdapters, LocalModelConfiguration, ModelProvider,
};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use support::{AcpProcess, BRIDGE};

const PRESET_ID: &str = "renoa.coding.alpha.v2";
const TURN_ID: &str = "7d33a1c0-5b52-4a44-9ed3-cb6e8a9f02f4";

#[test]
fn load_refuses_another_agents_executable_session() {
    let fixture = Fixture::provisioned();
    let foreign = settled_session(&fixture, &fixture.agent_b);
    let mut intruder = fixture.process(&fixture.agent_a);
    intruder.initialize();

    let (history, loaded) = intruder.load_session(&fixture.workspace, &foreign);

    assert_eq!(
        loaded["error"]["code"], -32602,
        "unexpected response: {loaded}"
    );
    assert!(
        loaded["error"]["data"]
            .as_str()
            .expect("refusal error data")
            .contains("session belongs to a different agent"),
        "unexpected response: {loaded}"
    );
    assert!(
        history.is_empty(),
        "another agent's history was replayed: {history:?}"
    );
    assert!(
        fixture.data.join("sessions").join(&foreign).is_dir(),
        "a refused load disturbed the foreign session"
    );
    intruder.finish();
}

#[test]
fn load_refuses_another_agents_session_when_only_history_is_available() {
    let fixture = Fixture::provisioned();
    let foreign = settled_session(&fixture, &fixture.agent_b);
    fs::remove_file(&fixture.bridge).expect("remove model adapter");
    let mut intruder = fixture.process(&fixture.agent_a);
    intruder.initialize();

    let (history, loaded) = intruder.load_session(&fixture.workspace, &foreign);

    assert_eq!(
        loaded["error"]["code"], -32602,
        "unexpected response: {loaded}"
    );
    assert!(
        loaded["error"]["data"]
            .as_str()
            .expect("refusal error data")
            .contains("session belongs to a different agent"),
        "unexpected response: {loaded}"
    );
    assert!(
        history.is_empty(),
        "another agent's history was replayed: {history:?}"
    );
    intruder.finish();
}

#[test]
fn load_still_admits_the_owning_agents_session_for_execution() {
    let fixture = Fixture::provisioned();
    let own = settled_session(&fixture, &fixture.agent_a);
    let mut resumed = fixture.process(&fixture.agent_a);
    resumed.initialize();

    let (history, loaded) = resumed.load_session(&fixture.workspace, &own);

    assert!(
        loaded["result"].is_object(),
        "own session must load: {loaded}"
    );
    let replay = history
        .iter()
        .find(|message| message["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .expect("replayed assistant turn");
    assert_eq!(
        replay["params"]["update"]["content"]["text"],
        "First response."
    );
    resumed.finish();
}

#[test]
fn load_still_admits_the_owning_agents_session_as_history() {
    let fixture = Fixture::provisioned();
    let own = settled_session(&fixture, &fixture.agent_a);
    fs::remove_file(&fixture.bridge).expect("remove model adapter");
    let mut resumed = fixture.process(&fixture.agent_a);
    resumed.initialize();

    let (history, loaded) = resumed.load_session(&fixture.workspace, &own);

    assert!(
        loaded["result"].is_object(),
        "own session history must load: {loaded}"
    );
    let unavailable = loaded["result"]["_meta"]["renoa.executionUnavailable"]
        .as_str()
        .expect("explicit execution status");
    assert!(!unavailable.is_empty());
    let replay = history
        .iter()
        .find(|message| message["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .expect("replayed assistant turn");
    assert_eq!(
        replay["params"]["update"]["content"]["text"],
        "First response."
    );
    resumed.send(
        &json!({"jsonrpc": "2.0", "id": 4, "method": "session/close", "params": {"sessionId": own}}),
    );
    assert!(resumed.read()["result"].is_object());
    resumed.finish();
}

#[test]
fn delete_refuses_another_agents_session_without_removing_it() {
    let fixture = Fixture::provisioned();
    let foreign = settled_session(&fixture, &fixture.agent_b);
    let mut intruder = fixture.process(&fixture.agent_a);
    intruder.initialize();

    let refused = delete_session(&mut intruder, 5, &foreign);
    assert_eq!(
        refused["error"]["code"], -32602,
        "unexpected response: {refused}"
    );
    assert!(
        refused["error"]["data"]
            .as_str()
            .expect("refusal error data")
            .contains("session belongs to a different agent"),
        "unexpected response: {refused}"
    );
    assert!(
        fixture.data.join("sessions").join(&foreign).is_dir(),
        "a refused delete removed the foreign session"
    );

    let absent = Uuid::new_v4().to_string();
    let missing = delete_session(&mut intruder, 6, &absent);
    assert!(
        missing["result"].is_object(),
        "an absent session must stay idempotently deletable: {missing}"
    );
    intruder.finish();
}

#[test]
fn delete_still_removes_the_owning_agents_session() {
    let fixture = Fixture::provisioned();
    let own = settled_session(&fixture, &fixture.agent_b);
    let mut owner = fixture.process(&fixture.agent_b);
    owner.initialize();

    let deleted = delete_session(&mut owner, 5, &own);
    assert!(
        deleted["result"].is_object(),
        "unexpected response: {deleted}"
    );
    assert!(!fixture.data.join("sessions").join(&own).exists());
    let repeated = delete_session(&mut owner, 6, &own);
    assert!(
        repeated["result"].is_object(),
        "unexpected response: {repeated}"
    );
    owner.finish();
}

fn delete_session(process: &mut AcpProcess, id: u64, session_id: &str) -> Value {
    process.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/delete",
        "params": { "sessionId": session_id }
    }));
    let response = process.read();
    assert_eq!(response["id"], id);
    response
}

struct Fixture {
    _directory: tempfile::TempDir,
    workspace: std::path::PathBuf,
    data: std::path::PathBuf,
    bridge: std::path::PathBuf,
    auth: std::path::PathBuf,
    agent_a: String,
    agent_b: String,
}

impl Fixture {
    fn provisioned() -> Self {
        let directory = tempdir().expect("fixture directory");
        let workspace = directory.path().join("workspace");
        let data = directory.path().join("data");
        let bridge = directory.path().join("bridge.mjs");
        let auth = directory.path().join("auth.sqlite");
        fs::create_dir(&workspace).expect("workspace");
        fs::write(&bridge, BRIDGE).expect("fixture model bridge");
        fs::write(&auth, "").expect("fixture credentials");
        let agent_a = provision(&data, &bridge, &auth, "Alpha");
        let agent_b = provision(&data, &bridge, &auth, "Beta");
        Self {
            _directory: directory,
            workspace,
            data,
            bridge,
            auth,
            agent_a,
            agent_b,
        }
    }

    fn process(&self, agent_id: &str) -> AcpProcess {
        AcpProcess::spawn_for_agent(
            &self.workspace,
            &self.data,
            &self.bridge,
            &self.auth,
            agent_id,
        )
    }
}

/// Settles one prompted session owned by `agent_id` and returns its id.
fn settled_session(fixture: &Fixture, agent_id: &str) -> String {
    let mut process = fixture.process(agent_id);
    process.initialize();
    let created = process.create_session(&fixture.workspace);
    let id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    assert_eq!(
        process.prompt(&id, "First", TURN_ID).1["result"]["stopReason"],
        "end_turn"
    );
    process.finish();
    id
}

/// Provisions one named agent in the shared data directory.
fn provision(data: &Path, bridge: &Path, auth: &Path, name: &str) -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("provisioning runtime");
    let agent_id = runtime.block_on(async {
        let host = LocalHost::new(
            data,
            LocalModelConfiguration::new(
                bridge,
                vec![ModelProvider::Xai],
                ModelProvider::Xai,
                "grok-test",
                auth,
            ),
            LocalHostAdapters::default(),
        )
        .expect("provisioning Host");
        host.create_agent(
            AgentCreator::System {
                component: "agent-boundary-test".to_owned(),
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
