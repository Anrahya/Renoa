use std::{fs, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_local::{
    AgentProfile, AgentProfileId, LocalHost, LocalHostAdapters, LocalHostError,
    LocalModelConfiguration, LocalTurnOutcome, ModelProvider, alpha_profile,
};
use rusqlite::Connection;
use tempfile::tempdir;
use uuid::Uuid;

const RELAY_PROFILE_ID: &str = "renoa.messaging.relay.v1";
const RELAY_PROMPT: &str = "You are Relay, a concise messaging agent.";

#[tokio::test]
async fn host_assembles_and_restores_an_exact_non_alpha_profile() {
    let directory = tempdir().expect("temporary Host directory");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("model-bridge.mjs");
    let credentials = directory.path().join("credentials.sqlite3");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&bridge, MODEL_BRIDGE).expect("write deterministic model bridge");
    fs::write(&credentials, "").expect("write credential placeholder");
    let relay_id = AgentProfileId::new(RELAY_PROFILE_ID).expect("valid Relay profile id");
    let host = local_host(&data, &bridge, &credentials, true);

    let session = host
        .create_session(&relay_id, &workspace)
        .await
        .expect("create Relay session");
    let session_id = session.id();
    let agent_id = session.agent_id();
    assert_eq!(session.profile_id(), &relay_id);
    assert_eq!(
        session
            .execute_turn(
                Uuid::new_v4(),
                vec![ContentBlock::text("Relay this message.")],
                Arc::new(NoopEvents),
            )
            .await
            .expect("run Relay profile"),
        LocalTurnOutcome::Completed {
            output: "Relay profile ran.".to_owned(),
            stop_reason: renoa_agent::StopReason::Stop,
        }
    );
    assert_trace_identity(&data, session_id, agent_id, &relay_id);
    drop(session);
    drop(host);

    let alpha_only = local_host(&data, &bridge, &credentials, false);
    let Err(error) = alpha_only.load_session(session_id, &workspace).await else {
        panic!("an unregistered profile must fail closed");
    };
    assert!(matches!(error, LocalHostError::InvalidRequest(_)));
    drop(alpha_only);

    let reopened = local_host(&data, &bridge, &credentials, true);
    let restored = reopened
        .load_session(session_id, &workspace)
        .await
        .expect("restore Relay session with its profile registered");
    assert_eq!(restored.profile_id(), &relay_id);
    assert_eq!(restored.agent_id(), agent_id);
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
    let relay_id = AgentProfileId::new(RELAY_PROFILE_ID).expect("valid Relay profile id");
    let requested_session = Uuid::new_v4();

    let host = local_host(&data, &bridge, &credentials, true);
    let created = host
        .ensure_session(&relay_id, &workspace, requested_session)
        .await
        .expect("create exact session");
    let agent_id = created.agent_id();
    assert_eq!(created.id(), requested_session);
    drop(created);
    let alpha_id = AgentProfileId::new(renoa_local::ALPHA_PROFILE_ID).expect("valid Alpha id");
    assert!(
        host.ensure_session(&alpha_id, &workspace, requested_session)
            .await
            .is_err()
    );
    drop(host);

    let reopened = local_host_with_model(
        &data,
        &bridge,
        &credentials,
        true,
        "not-a-new-session-model",
    );
    let restored = reopened
        .ensure_session(&relay_id, &workspace, requested_session)
        .await
        .expect("reuse exact session");
    assert_eq!(restored.id(), requested_session);
    assert_eq!(restored.agent_id(), agent_id);
    assert_eq!(restored.profile_id(), &relay_id);
}

fn local_host(data: &Path, bridge: &Path, credentials: &Path, with_relay: bool) -> LocalHost {
    local_host_with_model(data, bridge, credentials, with_relay, "fixture-model")
}

fn local_host_with_model(
    data: &Path,
    bridge: &Path,
    credentials: &Path,
    with_relay: bool,
    initial_model: &str,
) -> LocalHost {
    let mut profiles = vec![alpha_profile()];
    if with_relay {
        profiles
            .push(AgentProfile::new(RELAY_PROFILE_ID, RELAY_PROMPT).expect("valid Relay profile"));
    }
    LocalHost::new(
        data,
        LocalModelConfiguration::new(
            bridge,
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            initial_model,
            credentials,
        ),
        profiles,
        LocalHostAdapters::default(),
    )
    .expect("assemble local Host")
}

fn assert_trace_identity(
    data: &Path,
    session_id: Uuid,
    agent_id: renoa_kernel::AgentId,
    profile_id: &AgentProfileId,
) {
    let path = data
        .join("sessions")
        .join(session_id.to_string())
        .join("trace.sqlite3");
    let connection = Connection::open(path).expect("open trace database");
    let stored = connection
        .query_row(
            "SELECT session_id, agent_id, profile_id FROM trace_metadata",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .expect("read trace identity");
    assert_eq!(
        stored,
        (
            session_id.to_string(),
            agent_id.to_string(),
            profile_id.to_string()
        )
    );
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
  process.stderr.write("Host sent the wrong profile instructions");
  process.exit(3);
}
process.stdout.write(JSON.stringify({
  event: "completed",
  response: {
    content: [{ type: "text", text: "Relay profile ran." }],
    stop_reason: "stop",
    usage: { input: 8, output: 4, cache_read: 0, cache_write: 0 },
    metadata: { api: "test", provider: "xai", model: "fixture-model" }
  }
}) + "\n");
"#;
