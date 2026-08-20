use std::fs;

use renoa_local::{LocalRuntimeConfig, LocalWorkspace, build_local_runtime};
use tempfile::tempdir;

#[tokio::test]
async fn local_host_resolves_the_complete_coding_runtime() {
    let directory = tempdir().expect("temporary directory");
    let workspace_path = directory.path().join("workspace");
    let bridge = directory.path().join("bridge.mjs");
    let credentials = directory.path().join("credentials.sqlite3");
    fs::create_dir(&workspace_path).expect("create workspace");
    fs::write(&bridge, DESCRIBE_BRIDGE).expect("write bridge");
    fs::write(&credentials, "").expect("write credential placeholder");

    let workspace = LocalWorkspace::open(&workspace_path).expect("open workspace");
    let runtime = build_local_runtime(
        LocalRuntimeConfig::new(
            bridge,
            "xai",
            "grok-test",
            credentials,
            "You are a careful coding agent.",
        ),
        &workspace,
    )
    .await
    .expect("resolve local runtime");

    let manifest = runtime.manifest();
    assert_eq!(manifest.loop_binding, "renoa.agent.model-tool-loop");
    assert_eq!(manifest.checkpoint_schema_version, 2);
    assert_eq!(manifest.effect_bindings.len(), 7);
    assert!(manifest.effect_bindings.contains_key("renoa.agent.model"));
    for tool in [
        "read_file",
        "edit_file",
        "write_file",
        "bash",
        "grep",
        "find",
    ] {
        assert!(
            manifest
                .effect_bindings
                .contains_key(&format!("renoa.agent.tool/{tool}")),
            "missing full-access tool binding `{tool}`"
        );
    }
}

const DESCRIBE_BRIDGE: &str = r#"
if (process.env.RENOA_PI_ACTION !== "describe") {
  process.stderr.write("unexpected bridge action");
  process.exit(1);
}
process.stdout.write(JSON.stringify({
  ok: true,
  response: {
    context_window_tokens: 500000,
    max_output_tokens: 500000,
    model_spec: "{}",
    model_binding_id: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
    reasoning_level: "high"
  }
}));
"#;
