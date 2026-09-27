mod plugins;

use std::{collections::BTreeSet, fs, os::unix::fs::PermissionsExt as _, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use serde_json::json;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{HostInitialization, LocalHost};
use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalTurnOutcome,
    ModelProvider, PluginInvocation, PluginOutcome, PluginRequest, PluginSource,
    mcp::{McpAuthorizationResolver, McpCredentialResolver},
};

const TOKEN: &str = "fixture-shared-host-secret";

#[tokio::test]
async fn canonical_plugin_api_binds_one_existing_agent_and_reuses_shared_content() {
    let directory = tempdir().expect("fixture");
    let root = directory.path();
    prepare_fixture(root);
    let host = host(root);
    let first = create_agent(&host, "First").await;
    let second = create_agent(&host, "Second").await;
    let source = root.join("workspace/review");
    fs::create_dir(&source).expect("standalone skill");
    fs::write(
        source.join("SKILL.md"),
        "---\nname: review\ndescription: Review the source.\n---\nRead code first.\n",
    )
    .expect("skill instructions");
    let invocation = || PluginInvocation {
        operation_id: "host-plugin-fixture",
        updates: None,
        cancellation: CancellationToken::new(),
    };
    let workspace = root.join("workspace");
    let source = PluginSource::Skill {
        source_path: "review".into(),
    };
    let PluginOutcome::Inspected(inspected) = host
        .manage_plugin(
            &first,
            &workspace,
            PluginRequest::Inspect {
                source: source.clone(),
            },
            invocation(),
        )
        .await
        .expect("inspect through Host API")
    else {
        panic!("inspection outcome")
    };
    let install = || PluginRequest::Install {
        source: source.clone(),
        expected_digest: inspected.digest().to_owned(),
    };
    let unknown = crate::derived_agent_id(Uuid::new_v4());
    assert!(matches!(
        host.manage_plugin(&unknown, &workspace, install(), invocation())
            .await,
        Err(super::LocalHostError::AgentNotFound(id)) if id == unknown
    ));
    assert!(
        host.installed_plugins()
            .await
            .expect("no residue")
            .is_empty()
    );
    host.manage_plugin(&first, &workspace, install(), invocation())
        .await
        .expect("install into shared library");
    fs::remove_dir_all(workspace.join("review")).expect("remove mutable source");
    host.manage_plugin(&second, &workspace, install(), invocation())
        .await
        .expect("another agent can reuse exact content without original source");
    assert!(
        host.config
            .skill_store
            .summaries(&first.to_string(), &workspace)
            .expect("install does not enable skills")
            .is_empty()
    );
    host.manage_plugin(
        &second,
        &workspace,
        PluginRequest::Add {
            source: PluginSource::Installed {
                package_digest: inspected.digest().to_owned(),
            },
            expected_digest: None,
            server: None,
            connection: None,
            credential: None,
            replace: false,
        },
        invocation(),
    )
    .await
    .expect("enable skills only for the second agent");
    for (agent, expected) in [(first, 0), (second, 1)] {
        assert_eq!(
            host.config
                .skill_store
                .summaries(&agent.to_string(), &workspace)
                .expect("exact agent selection")
                .len(),
            expected
        );
    }
    assert_eq!(
        host.installed_plugins()
            .await
            .expect("shared library")
            .len(),
        1
    );
}

#[tokio::test]
async fn live_host_clients_reuse_credentials_packages_and_skills_across_agents_and_restart() {
    let directory = tempdir().expect("fixture");
    let root = directory.path();
    prepare_fixture(root);
    let first = host(root);
    let second = host(root);
    assert_eq!(
        first.host_id().await.expect("first identity"),
        second.host_id().await.expect("second identity")
    );
    let first_agent = create_agent(&first, "First").await;
    let second_agent = create_agent(&first, "Second").await;
    let session = second
        .ensure_agent_session(second_agent, &root.join("workspace"), Uuid::new_v4())
        .await
        .expect("live second session");
    let session_id = session.id();

    let digest = publish_capabilities(&first, root, first_agent).await;
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
            .agent_definition(second_agent)
            .await
            .expect("second agent definition")
            .expect("second agent")
            .connections,
        BTreeSet::from(["shared-x".to_owned()])
    );
    drop(session);
    drop(second);
    let reopened = host(root);
    let restored = reopened
        .load_session_for_agent(second_agent, session_id, &root.join("workspace"))
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
        2
    );
    for path in [
        root.join("data/state/host.sqlite3"),
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
        code_mode: None,
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

async fn create_agent(host: &LocalHost, name: &str) -> crate::AgentId {
    host.create_agent(
        AgentCreator::System {
            component: "shared-capabilities".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        AgentCreateRequest::from_preset(
            Uuid::new_v4(),
            AgentPresetId::new(crate::presets::ALPHA_PRESET_ID).expect("preset"),
            name,
        ),
        CancellationToken::new(),
    )
    .await
    .expect("agent")
    .id
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
        concat!(
            include_str!("../../tests/support/plugin_driver.mjs"),
            include_str!("shared_capabilities_model.mjs")
        ),
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

async fn publish_capabilities(first: &LocalHost, root: &Path, agent: crate::AgentId) -> String {
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
        .enable_agent_connection(agent, "shared-x")
        .await
        .expect("enable first agent");
    inspected.digest().to_owned()
}
