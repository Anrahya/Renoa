use std::{fs, os::unix::fs::PermissionsExt as _, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

use super::{HostInitialization, LocalHost};
use crate::{
    AgentProfile, AgentProfileId, LocalTurnOutcome, ModelProvider,
    mcp::{McpAuthorizationResolver, McpCredentialResolver},
};

const FIRST: &str = "shared.first";
const SECOND: &str = "shared.second";
const TOKEN: &str = "fixture-shared-host-secret";

#[tokio::test]
async fn live_host_clients_reuse_credentials_packages_and_skills_across_profiles_and_restart() {
    let directory = tempdir().expect("fixture");
    let root = directory.path();
    prepare_fixture(root);
    let first = host(root);
    let second = host(root);
    assert_eq!(
        first.host_id().await.expect("first identity"),
        second.host_id().await.expect("second identity")
    );
    let profile = AgentProfileId::new(SECOND).expect("profile");
    let session = second
        .create_session(&profile, &root.join("workspace"))
        .await
        .expect("live second session");
    let session_id = session.id();

    let digest = publish_capabilities(&first, root).await;
    let expected = LocalTurnOutcome::Completed {
        output: "Shared capabilities ready.".to_owned(),
        stop_reason: renoa_agent::StopReason::Stop,
    };
    assert_eq!(
        session
            .execute_turn(
                Uuid::new_v4(),
                vec![ContentBlock::text(format!("Reuse {digest}"))],
                Arc::new(Noop)
            )
            .await
            .expect("reuse through the second live agent"),
        expected
    );
    assert_eq!(
        second
            .profile_mcp_connection_ids(&profile)
            .await
            .expect("second profile connections"),
        ["shared-x"]
    );
    drop(session);
    drop(second);
    let reopened = host(root);
    let restored = reopened
        .load_session(session_id, &root.join("workspace"))
        .await
        .expect("restore exact second session");
    assert_eq!(
        restored
            .execute_turn(
                Uuid::new_v4(),
                vec![ContentBlock::text("Confirm")],
                Arc::new(Noop)
            )
            .await
            .expect("reuse after restart"),
        expected
    );
    let calls = fs::read_to_string(root.join("mcp-calls")).expect("MCP calls");
    let model_sessions = fs::read_to_string(root.join("model-sessions")).expect("model sessions");
    assert!(!model_sessions.is_empty());
    assert!(
        model_sessions
            .lines()
            .all(|id| id == session_id.to_string())
    );
    assert_eq!(calls.lines().filter(|line| *line == "discover").count(), 1);
    assert_eq!(calls.lines().filter(|line| *line == "call").count(), 2);
    assert_eq!(
        first
            .installed_plugins()
            .await
            .expect("same package library")
            .len(),
        1
    );
    for path in [
        root.join("data/host.sqlite3"),
        root.join(format!("data/sessions/{session_id}/kernel.sqlite3")),
    ] {
        let bytes = fs::read(path).expect("durable database");
        assert!(
            !bytes
                .windows(TOKEN.len())
                .any(|window| window == TOKEN.as_bytes())
        );
    }
}

fn host(root: &Path) -> LocalHost {
    let mut host = LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::Xai],
        initial_provider: ModelProvider::Xai,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("model-auth.sqlite"),
        mcp_adapter: Some(root.join("mcp.mjs")),
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: Some(root.join("global")),
        oauth_relay: None,
        profiles: [FIRST, SECOND]
            .into_iter()
            .map(|id| AgentProfile::new(id, "Test shared Host capabilities.").expect("profile"))
            .collect(),
    })
    .expect("Host client");
    let config = Arc::get_mut(&mut host.config).expect("exclusive new configuration");
    config.mcp_authorizations = McpAuthorizationResolver::new(
        &config.mcp_catalog,
        config.mcp_adapter.clone(),
        McpCredentialResolver::with_gh_executable(root.join("gh")),
    );
    host
}

struct Noop;
impl AgentEventSink for Noop {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

fn prepare_fixture(root: &Path) {
    fs::create_dir(root.join("workspace")).expect("workspace");
    fs::create_dir(root.join("global")).expect("global skills");
    fs::write(root.join("model-auth.sqlite"), "").expect("model credentials boundary");
    fs::write(
        root.join("model.mjs"),
        include_str!("shared_capabilities_model.mjs"),
    )
    .expect("model boundary");
    fs::write(
        root.join("mcp.mjs"),
        include_str!("shared_capabilities_mcp.mjs"),
    )
    .expect("MCP boundary");
    fs::write(
        root.join("gh"),
        format!("#!/bin/sh\nprintf '%s\\n' '{TOKEN}'\n"),
    )
    .expect("credential boundary");
    fs::set_permissions(root.join("gh"), fs::Permissions::from_mode(0o700))
        .expect("credential executable");
}

async fn publish_capabilities(first: &LocalHost, root: &Path) -> String {
    let source = root.join("package");
    fs::create_dir_all(source.join("skills/shared-workflow")).expect("package skill");
    fs::write(
        source.join("plugin.json"),
        json!({
            "$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name":"shared-package"
        })
        .to_string(),
    )
    .expect("manifest");
    fs::write(source.join("skills/shared-workflow/SKILL.md"),
        "---\nname: shared-workflow\ndescription: Shared Host workflow.\n---\nSHARED_HOST_SKILL_INSTRUCTIONS\n"
    ).expect("skill");
    let inspected = first.inspect_plugin(&source).await.expect("inspect once");
    first
        .install_plugin(&source, inspected.digest())
        .await
        .expect("install once");
    fs::remove_dir_all(&source).expect("original package is no longer needed");
    first
        .register_gh_cli_mcp_connection(
            "shared-service",
            "shared-x",
            "https://example.com/mcp",
            "github.com",
            "fixture-owner",
        )
        .await
        .expect("one authenticated Host connection");
    first
        .refresh_mcp_catalog("shared-x")
        .await
        .expect("discover once");
    first
        .enable_profile_mcp_connection(
            &AgentProfileId::new(FIRST).expect("first profile"),
            "shared-x",
        )
        .await
        .expect("enable first profile");
    inspected.digest().to_owned()
}
