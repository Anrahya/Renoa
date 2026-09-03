use std::fs;

use renoa_local::{
    ALPHA_PROFILE_ID, ARCEE_PROFILE_ID, AgentProfileId, LocalHost, LocalHostAdapters,
    LocalHostError, LocalModelConfiguration, ModelProvider, ReasoningLevel, alpha_profile,
    arcee_profile,
};
use tempfile::tempdir;

#[tokio::test]
async fn an_open_agent_session_can_select_a_newly_discovered_model() {
    let directory = tempdir().expect("temporary directory");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("bridge.mjs");
    let catalog_calls = directory.path().join("catalog-calls");
    let credentials = directory.path().join("credentials.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&credentials, "").expect("create credential placeholder");
    fs::write(
        &bridge,
        bridge_source(catalog_calls.to_string_lossy().as_ref()),
    )
    .expect("write model bridge");
    let host = LocalHost::new(
        data,
        LocalModelConfiguration::new(
            &bridge,
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "model-a",
            &credentials,
        ),
        vec![alpha_profile()],
        LocalHostAdapters::default(),
    )
    .expect("assemble Host");

    let session = host
        .create_session(
            &AgentProfileId::new(ALPHA_PROFILE_ID).expect("Alpha profile id"),
            &workspace,
        )
        .await
        .expect("create Alpha session from first catalog");
    let initial = session.configuration().expect("initial configuration");
    assert_eq!(
        initial
            .models
            .iter()
            .map(renoa_local::ModelChoice::selection_id)
            .collect::<Vec<_>>(),
        ["xai/model-a"]
    );

    let refreshed = session
        .refresh_configuration()
        .await
        .expect("refresh the authenticated model catalog");
    assert_eq!(refreshed.model, "xai/model-a");
    assert_eq!(
        refreshed
            .models
            .iter()
            .map(renoa_local::ModelChoice::selection_id)
            .collect::<Vec<_>>(),
        ["xai/model-a", "xai/model-b"]
    );

    session
        .set_model("xai/model-b")
        .await
        .expect("select the newly advertised model");
    assert_eq!(
        session
            .configuration()
            .expect("selected configuration")
            .model,
        "xai/model-b"
    );

    let error = session
        .set_model("xai/not-real")
        .await
        .expect_err("Renoa remains authoritative for a fresh picker value");
    assert!(matches!(error, LocalHostError::InvalidRequest(_)));
    assert_eq!(
        session
            .configuration()
            .expect("rejected selection preserves configuration")
            .model,
        "xai/model-b"
    );
}

#[tokio::test]
async fn arcee_exposes_only_opencode_go_even_when_the_host_has_other_providers() {
    let directory = tempdir().expect("temporary directory");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("bridge.mjs");
    let catalog_calls = directory.path().join("catalog-calls");
    let credentials = directory.path().join("credentials.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&credentials, "").expect("create credential placeholder");
    fs::write(
        &bridge,
        bridge_source(catalog_calls.to_string_lossy().as_ref()),
    )
    .expect("write model bridge");
    let profile = arcee_profile(&data).expect("create Arcee profile");
    let host = LocalHost::new(
        &data,
        LocalModelConfiguration::new(
            &bridge,
            vec![ModelProvider::Xai, ModelProvider::OpenCodeGo],
            ModelProvider::Xai,
            "model-a",
            &credentials,
        )
        .with_initial_reasoning(ReasoningLevel::Xhigh),
        vec![profile],
        LocalHostAdapters::default(),
    )
    .expect("assemble Host");

    let session = host
        .create_session(
            &AgentProfileId::new(ARCEE_PROFILE_ID).expect("Arcee profile id"),
            &workspace,
        )
        .await
        .expect("create Arcee session");
    let configuration = session.configuration().expect("Arcee configuration");
    assert_eq!(configuration.model, "opencode-go/model-a");
    assert_eq!(configuration.reasoning, ReasoningLevel::Xhigh);
    assert!(
        configuration
            .models
            .iter()
            .all(|model| model.provider() == ModelProvider::OpenCodeGo)
    );
    let error = session
        .set_model("xai/model-a")
        .await
        .expect_err("Arcee must not switch to a provider outside its profile");
    assert!(matches!(error, LocalHostError::InvalidRequest(_)));
}

#[tokio::test]
async fn configured_initial_reasoning_must_be_supported_by_the_model() {
    let directory = tempdir().expect("temporary directory");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("bridge.mjs");
    let catalog_calls = directory.path().join("catalog-calls");
    let credentials = directory.path().join("credentials.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&credentials, "").expect("create credential placeholder");
    fs::write(
        &bridge,
        bridge_source(catalog_calls.to_string_lossy().as_ref()),
    )
    .expect("write model bridge");
    let host = LocalHost::new(
        data,
        LocalModelConfiguration::new(
            &bridge,
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "model-a",
            &credentials,
        )
        .with_initial_reasoning(ReasoningLevel::Max),
        vec![alpha_profile()],
        LocalHostAdapters::default(),
    )
    .expect("assemble Host");

    let error = host
        .create_session(
            &AgentProfileId::new(ALPHA_PROFILE_ID).expect("Alpha profile id"),
            &workspace,
        )
        .await
        .err()
        .expect("reject unsupported initial reasoning");

    assert!(matches!(error, LocalHostError::Configuration(_)));
    assert_eq!(
        error.to_string(),
        "invalid local Host configuration: configured xai/model-a model does not support max reasoning"
    );
}

fn bridge_source(catalog_calls: &str) -> String {
    format!(
        r#"
import {{ createHash }} from "node:crypto";
import {{ existsSync, readFileSync, writeFileSync }} from "node:fs";
const action = process.env.RENOA_MODEL_ACTION;
if (action === "catalog") {{
  const path = {catalog_calls};
  const calls = existsSync(path) ? Number(readFileSync(path, "utf8")) : 0;
  writeFileSync(path, String(calls + 1));
  const ids = calls === 0 ? ["model-a"] : ["model-a", "model-b"];
  process.stdout.write(JSON.stringify({{ ok: true, response: {{ models: ids.map(id => ({{
    id,
    name: id === "model-a" ? "Model A" : "Model B",
    reasoning_levels: ["high", "xhigh"],
    context_window_tokens: 1000000,
    model_spec: {{ id }}
  }})) }} }}));
  process.exit(0);
}}
if (action === "describe") {{
  const modelSpec = process.env.RENOA_MODEL_SPEC;
  process.stdout.write(JSON.stringify({{ ok: true, response: {{
    context_window_tokens: 1000000,
    max_output_tokens: 8192,
    model_spec: modelSpec,
    model_binding_id: createHash("sha256").update(modelSpec).digest("hex"),
    reasoning_level: process.env.RENOA_MODEL_REASONING
  }} }}));
  process.exit(0);
}}
process.exit(2);
"#,
        catalog_calls = serde_json::to_string(catalog_calls).expect("encode counter path")
    )
}
