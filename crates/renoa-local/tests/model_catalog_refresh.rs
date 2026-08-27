use std::fs;

use renoa_local::{LocalHost, LocalHostError, ModelProvider};
use tempfile::tempdir;

#[tokio::test]
async fn an_open_alpha_session_can_select_a_newly_discovered_model() {
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
        &bridge,
        vec![ModelProvider::Xai],
        ModelProvider::Xai,
        "model-a",
        &credentials,
        None,
    )
    .expect("assemble Host");

    let session = host
        .create_alpha_session(&workspace)
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

    session
        .set_model("xai/model-b")
        .await
        .expect("refresh and select the newly advertised model");
    let refreshed = session.configuration().expect("refreshed configuration");
    assert_eq!(refreshed.model, "xai/model-b");
    assert_eq!(
        refreshed
            .models
            .iter()
            .map(renoa_local::ModelChoice::selection_id)
            .collect::<Vec<_>>(),
        ["xai/model-a", "xai/model-b"]
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
    reasoning_levels: ["high"],
    context_window_tokens: 100000,
    model_spec: {{ id }}
  }})) }} }}));
  process.exit(0);
}}
if (action === "describe") {{
  const modelSpec = process.env.RENOA_MODEL_SPEC;
  process.stdout.write(JSON.stringify({{ ok: true, response: {{
    context_window_tokens: 100000,
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
