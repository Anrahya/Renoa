use std::{fs, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentDefinition, AgentId, AgentPresetId,
    LocalHost, LocalHostAdapters, LocalHostError, LocalModelConfiguration, LocalTurnOutcome,
    ModelProvider,
};
use rusqlite::Connection;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const SPECIALIST_PRESET_ID: &str = "renoa.specialist.v2";
const RELAY_PROMPT: &str = "You are Relay, a concise messaging agent.";

async fn provision_specialist(
    host: &LocalHost,
    operation: Uuid,
    name: &str,
    instructions: &str,
) -> AgentDefinition {
    host.create_agent(
        AgentCreator::System {
            component: "multi-agent-test".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        AgentCreateRequest::new(
            operation,
            AgentPresetId::new(SPECIALIST_PRESET_ID).expect("valid specialist preset id"),
            name,
        )
        .with_instructions(instructions),
        CancellationToken::new(),
    )
    .await
    .expect("agent")
}

#[tokio::test]
async fn host_assembles_and_restores_an_exact_non_alpha_agent() {
    let directory = tempdir().expect("temporary Host directory");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("model-bridge.mjs");
    let credentials = directory.path().join("credentials.sqlite3");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&bridge, MODEL_BRIDGE).expect("write deterministic model bridge");
    fs::write(&credentials, "").expect("write credential placeholder");
    let host = local_host(&data, &bridge, &credentials);

    let relay = provision_specialist(&host, Uuid::new_v4(), "Relay", RELAY_PROMPT).await;
    let session = host
        .ensure_agent_session(relay.id, &workspace, Uuid::new_v4())
        .await
        .expect("create Relay session");
    let session_id = session.id();
    let agent_id = session.agent_id();
    assert_eq!(agent_id, relay.id);
    assert_eq!(
        session
            .execute_turn(
                Uuid::new_v4(),
                vec![ContentBlock::text("Relay this message.")],
                Arc::new(NoopEvents),
            )
            .await
            .expect("run Relay agent"),
        LocalTurnOutcome::Completed {
            output: "Relay agent ran.".to_owned(),
            stop_reason: renoa_agent::StopReason::Stop,
        }
    );
    assert_trace_identity(&data, session_id, agent_id);
    drop(session);
    drop(host);

    let reopened = local_host(&data, &bridge, &credentials);
    let restored = reopened
        .load_session_for_agent(agent_id, session_id, &workspace)
        .await
        .expect("restore Relay session with its agent registered");
    assert_eq!(restored.id(), session_id);
    assert_eq!(restored.agent_id(), agent_id);
    drop(restored);

    let catalog = Connection::open(data.join("host.sqlite3")).expect("open catalog");
    catalog
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("detach the corruption fixture from dependent rows");
    for table in ["host_agent_creations", "host_agent_tool_selections"] {
        catalog
            .execute(
                &format!("DELETE FROM {table} WHERE agent_id = ?1"),
                [agent_id.to_string()],
            )
            .expect("remove dependent rows");
    }
    catalog
        .execute(
            "DELETE FROM host_agents WHERE agent_id = ?1",
            [agent_id.to_string()],
        )
        .expect("remove the agent row");
    let Err(error) = reopened
        .load_session_for_agent(agent_id, session_id, &workspace)
        .await
    else {
        panic!("an unregistered agent must fail closed");
    };
    assert!(
        matches!(error, LocalHostError::AgentNotFound(id) if id == agent_id),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn a_surface_can_retry_one_exact_session_identity_after_restart() {
    let directory = tempdir().expect("temporary Host directory");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("model-bridge.mjs");
    let credentials = directory.path().join("credentials.sqlite3");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&bridge, MODEL_BRIDGE).expect("write deterministic model bridge");
    fs::write(&credentials, "").expect("write credential placeholder");
    let requested_session = Uuid::new_v4();

    let host = local_host(&data, &bridge, &credentials);
    let relay = provision_specialist(&host, Uuid::new_v4(), "Relay", RELAY_PROMPT).await;
    let created = host
        .ensure_agent_session(relay.id, &workspace, requested_session)
        .await
        .expect("create exact session");
    let agent_id = created.agent_id();
    assert_eq!(created.id(), requested_session);
    drop(created);
    let other = provision_specialist(&host, Uuid::new_v4(), "Other", "Do something else.").await;
    assert!(
        host.ensure_agent_session(other.id, &workspace, requested_session)
            .await
            .is_err(),
        "a session identity cannot move to another agent"
    );
    drop(host);

    let reopened = local_host_with_model(&data, &bridge, &credentials, "not-a-new-session-model");
    let restored = reopened
        .ensure_agent_session(agent_id, &workspace, requested_session)
        .await
        .expect("reuse exact session");
    assert_eq!(restored.id(), requested_session);
    assert_eq!(restored.agent_id(), agent_id);
}

fn local_host(data: &Path, bridge: &Path, credentials: &Path) -> LocalHost {
    local_host_with_model(data, bridge, credentials, "fixture-model")
}

fn local_host_with_model(
    data: &Path,
    bridge: &Path,
    credentials: &Path,
    initial_model: &str,
) -> LocalHost {
    LocalHost::new(
        data,
        LocalModelConfiguration::new(
            bridge,
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            initial_model,
            credentials,
        ),
        LocalHostAdapters::default(),
    )
    .expect("assemble local Host")
}

fn assert_trace_identity(data: &Path, session_id: Uuid, agent_id: AgentId) {
    let path = data
        .join("sessions")
        .join(session_id.to_string())
        .join("trace.sqlite3");
    let connection = Connection::open(path).expect("open trace database");
    let stored = connection
        .query_row(
            "SELECT session_id, agent_id FROM trace_metadata",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("read trace identity");
    assert_eq!(stored, (session_id.to_string(), agent_id.to_string()));
}

struct NoopEvents;

impl AgentEventSink for NoopEvents {
    fn emit(&self, _event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

const MODEL_BRIDGE: &str = r#"
import { createHash } from "node:crypto";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const action = process.env.RENOA_MODEL_ACTION;
const modelSpec = JSON.stringify({ id: "fixture-model" });
if (action === "catalog") {
  process.stdout.write(JSON.stringify({ ok: true, response: { models: [{
    id: "fixture-model",
    name: "Fixture Model",
    reasoning_levels: ["high"],
    context_window_tokens: 100000,
    model_spec: { id: "fixture-model" }
  }] } }));
  process.exit(0);
}
if (action === "describe") {
  process.stdout.write(JSON.stringify({ ok: true, response: {
    context_window_tokens: 100000,
    max_output_tokens: 8192,
    model_spec: modelSpec,
    model_binding_id: createHash("sha256").update(modelSpec).digest("hex"),
    reasoning_level: "high"
  } }));
  process.exit(0);
}
if (action !== "stream") process.exit(2);
const request = JSON.parse(input);
if (request.system_prompt !== "You are Relay, a concise messaging agent.") {
  process.stderr.write("Host sent the wrong agent instructions");
  process.exit(3);
}
process.stdout.write(JSON.stringify({
  event: "completed",
  response: {
    content: [{ type: "text", text: "Relay agent ran." }],
    stop_reason: "stop",
    usage: { input: 8, output: 4, cache_read: 0, cache_write: 0 },
    metadata: { api: "test", provider: "xai", model: "fixture-model" }
  }
}) + "\n");
"#;

#[path = "multi_profile_host/cancellation.rs"]
mod cancellation;

#[path = "multi_profile_host/agents.rs"]
mod agents;
