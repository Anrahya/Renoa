use std::fs;

use renoa_kernel::RuntimeManifest;
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalHost,
    LocalHostAdapters, LocalModelConfiguration, LocalRuntimeConfig, LocalWorkspace, ModelProvider,
    build_local_runtime,
};
use tempfile::tempdir;

fn assert_workspace_bindings(manifest: &RuntimeManifest) {
    for tool in [
        "read_file",
        "edit_file",
        "write_file",
        "bash",
        "grep",
        "find",
        "git_changes",
        "git_diff",
        "git_show",
    ] {
        assert!(
            manifest
                .effect_bindings
                .contains_key(&format!("renoa.agent.tool/{tool}")),
            "missing selected tool binding `{tool}`"
        );
    }
}

/// The runtime a Host composes comes from the agent's durable definition, and a
/// workspace-rule change is visible to the next composition.
#[tokio::test]
async fn the_host_composes_the_coding_runtime_from_the_stored_definition() {
    let directory = tempdir().expect("temporary directory");
    let workspace_path = directory.path().join("workspace");
    let bridge = directory.path().join("bridge.mjs");
    let credentials = directory.path().join("credentials.sqlite3");
    fs::create_dir(&workspace_path).expect("create workspace");
    fs::write(
        workspace_path.join("AGENTS.md"),
        "Keep the host composition deterministic.\n",
    )
    .expect("write project instructions");
    fs::write(&bridge, DESCRIBE_BRIDGE).expect("write bridge");
    fs::write(&credentials, "").expect("write credential placeholder");

    let host = LocalHost::new(
        directory.path().join("data"),
        LocalModelConfiguration::new(
            bridge.clone(),
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "grok-test",
            credentials.clone(),
        ),
        LocalHostAdapters::new(None),
    )
    .expect("assemble Host");
    let agent = host
        .create_agent(
            AgentCreator::System {
                component: "composition-test".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                uuid::Uuid::new_v4(),
                AgentPresetId::new("renoa.coding.alpha.v1").expect("preset id"),
                "Local",
            ),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("provision the coding agent");

    let workspace = LocalWorkspace::open(&workspace_path).expect("open workspace");
    let resolved = host
        .resolve_definition(agent.id)
        .await
        .expect("resolve the stored definition");
    let captured = LocalRuntimeConfig::for_definition(
        bridge.clone(),
        "xai",
        "grok-test",
        credentials.clone(),
        &resolved,
        &workspace,
    )
    .expect("capture the agent runtime configuration");
    fs::write(
        workspace_path.join("AGENTS.md"),
        "Use the changed project instructions.\n",
    )
    .expect("change project instructions after capture");
    let runtime = build_local_runtime(captured, &workspace)
        .await
        .expect("resolve local runtime");

    let manifest = runtime.manifest();
    assert_eq!(manifest.loop_binding, "renoa.agent.model-tool-loop");
    assert_eq!(manifest.checkpoint_schema_version, 4);
    assert!(manifest.effect_bindings.contains_key("renoa.agent.model"));
    assert_workspace_bindings(manifest);

    let recomposed = host
        .resolve_definition(agent.id)
        .await
        .expect("re-resolve the stored definition");
    let changed = build_local_runtime(
        LocalRuntimeConfig::for_definition(
            bridge,
            "xai",
            "grok-test",
            credentials,
            &recomposed,
            &workspace,
        )
        .expect("recompose the agent runtime configuration"),
        &workspace,
    )
    .await
    .expect("resolve runtime after instruction change");
    assert_ne!(
        manifest.config_digest,
        changed.manifest().config_digest,
        "the next composition must read the changed workspace rules"
    );
}

const DESCRIBE_BRIDGE: &str = r#"
if (process.env.RENOA_MODEL_ACTION !== "describe") {
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
