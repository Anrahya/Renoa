use std::{collections::BTreeSet, fs};

use renoa_agent::{ContentBlock, Message, ModelRequest, ToolSpec};
use renoa_agent_loop::AgentToolBinding;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    offered_tool_bindings, partition_selected_tools, synthetic_activity_tools_for_definition,
};
use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalWorkspace,
    ModelProvider,
    host::{HostInitialization, LocalHost},
    model_context::estimate_input_tokens,
    presets::ARCEE_PRESET_ID,
};

#[tokio::test]
async fn every_non_workspace_catalog_component_has_one_runtime_binding() {
    let directory = tempfile::tempdir().expect("fixture");
    let root = directory.path();
    fs::create_dir(root.join("workspace")).expect("workspace");
    fs::write(root.join("model.mjs"), "// fixture\n").expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
    let host = LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::OpenCodeGo],
        initial_provider: ModelProvider::OpenCodeGo,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("auth.sqlite"),
        mcp_adapter: None,
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: None,
        oauth_relay: None,
        code_mode: None,
    })
    .expect("Host");
    let definition = host
        .create_agent(
            AgentCreator::System {
                component: "catalog-test".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
                "Catalog",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("agent");
    let resolved = host
        .resolve_definition(definition.id)
        .await
        .expect("definition");
    let workspace = LocalWorkspace::open(root.join("workspace")).expect("open workspace");
    let offer = |resolved: &crate::host::definition::ResolvedAgentDefinition| {
        offered_tool_bindings(
            &host.config,
            resolved,
            &workspace,
            renoa_kernel::SessionId::from_uuid(Uuid::new_v4()),
            None,
            crate::speaker::ToolAccess::Granted,
        )
        .expect("plugin runtime")
    };
    let platform = ["plugin_manage", "plugin_search", "tool_execute"];
    let resident = [
        "follow_up",
        "agent_documents",
        "memory_note",
        "memory_recall",
        "memory_zoom",
        "ask_owner",
        "notify_owner",
        "set_today",
        "share_file",
        "stop_sharing",
    ];
    assert_eq!(
        offer(&resolved)
            .iter()
            .map(AgentToolBinding::tool_name)
            .collect::<BTreeSet<_>>(),
        platform
            .into_iter()
            .chain(resident)
            .filter(|tool| !tool.starts_with("memory_"))
            .collect(),
        "a turn with no person lists no notebook tool"
    );
    let offered = offer(&resolved.with_principal(Some(Uuid::new_v4())));
    let actual: BTreeSet<&str> = offered.iter().map(AgentToolBinding::tool_name).collect();
    assert_eq!(
        actual.len(),
        offered.len(),
        "runtime tool names must be unique"
    );
    assert_eq!(actual, platform.into_iter().chain(resident).collect());
    // Code Mode hides only the executor: resident tools stay direct.
    let (visible, hidden) =
        partition_selected_tools(offered, true).expect("partition selected tools");
    assert_eq!(
        visible
            .iter()
            .map(AgentToolBinding::tool_name)
            .collect::<Vec<_>>(),
        [&["plugin_search", "plugin_manage"][..], &resident[..]].concat()
    );
    assert_eq!(
        hidden.expect("hidden MCP executor").tool_name(),
        "tool_execute"
    );
}

#[tokio::test]
async fn synthetic_activity_tools_and_resolved_arcee_token_cost_match_runtime() {
    let directory = tempfile::tempdir().expect("fixture");
    let root = directory.path();
    fs::create_dir(root.join("workspace")).expect("workspace");
    fs::write(root.join("model.mjs"), "// fixture\n").expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
    let host = LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::OpenCodeGo],
        initial_provider: ModelProvider::OpenCodeGo,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("auth.sqlite"),
        mcp_adapter: None,
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: None,
        oauth_relay: None,
        code_mode: None,
    })
    .expect("Host");
    let definition = host
        .create_agent(
            AgentCreator::System {
                component: "activity-cost-test".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
                "Arcee",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("agent");
    let resolved = host
        .resolve_definition(definition.id)
        .await
        .expect("definition");
    let workspace = LocalWorkspace::open(root.join("workspace")).expect("open workspace");
    let session_id = renoa_kernel::SessionId::from_uuid(Uuid::new_v4());

    let synthetic =
        synthetic_activity_tools_for_definition(&host.config, &resolved, &workspace, session_id)
            .expect("synthetic activity tools");
    assert!(synthetic.contains("read_file"));
    assert!(synthetic.contains("memory_note"));
    assert!(!synthetic.contains("unregistered_tool"));

    let selected_tools = resolved.selected_tools().tools.clone();
    let system_prompt = resolved
        .system_prompt(&root.join("workspace"))
        .expect("system prompt");
    let offered = offered_tool_bindings(
        &host.config,
        &resolved.with_principal(Some(Uuid::new_v4())),
        &workspace,
        session_id,
        None,
        crate::speaker::ToolAccess::Granted,
    )
    .expect("plugin runtime");
    let mut base_specs: Vec<ToolSpec> = workspace
        .selected_kernel_tool_bindings(Some(&selected_tools))
        .into_iter()
        .map(|binding| binding.tool().spec().clone())
        .chain(offered.iter().map(|binding| binding.tool().spec().clone()))
        .collect();
    let base_count = u64::try_from(base_specs.len()).expect("tool count fits u64");
    let with_activity: Vec<ToolSpec> = base_specs
        .iter()
        .map(ToolSpec::with_activity_field)
        .collect();
    let messages = vec![Message::User {
        content: vec![ContentBlock::text("Check workspace status")],
    }];
    let tokens_before = estimate_input_tokens(&ModelRequest {
        system_prompt: system_prompt.clone(),
        messages: messages.clone(),
        tools: std::mem::take(&mut base_specs),
    });
    let tokens_after = estimate_input_tokens(&ModelRequest {
        system_prompt,
        messages,
        tools: with_activity,
    });
    let delta_tokens = tokens_after - tokens_before;
    assert!(
        delta_tokens > 0 && delta_tokens <= base_count * 45,
        "unexpected resolved arcee token overhead {delta_tokens} across {base_count} tools"
    );
}

struct NoOneElse;

impl crate::AgentPeers for NoOneElse {
    fn roster(
        &self,
        _: renoa_kernel::AgentId,
    ) -> renoa_agent::BoxFuture<'_, Result<Vec<crate::PeerAgent>, crate::PeerError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn send(
        &self,
        _: crate::PeerMessage,
    ) -> renoa_agent::BoxFuture<'_, Result<(), crate::PeerError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn messaging_tools_are_offered_only_once_a_node_attaches_peers_and_follow_the_switch() {
    let directory = tempfile::tempdir().expect("fixture");
    let root = directory.path();
    fs::write(root.join("model.mjs"), "// fixture\n").expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
    let host = LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::OpenCodeGo],
        initial_provider: ModelProvider::OpenCodeGo,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("auth.sqlite"),
        mcp_adapter: None,
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: None,
        oauth_relay: None,
        code_mode: None,
    })
    .expect("Host");
    let agent = host
        .create_agent(
            AgentCreator::System {
                component: "peers-test".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
                "Messenger",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("agent")
        .id;
    let offered = |host: &LocalHost| {
        super::peer_bindings(&host.config, agent, None)
            .iter()
            .map(|binding| binding.tool_name().to_owned())
            .collect::<Vec<_>>()
    };

    assert!(
        offered(&host).is_empty(),
        "a Host with no node offers no messaging tool"
    );
    host.attach_agent_peers(std::sync::Arc::new(NoOneElse));
    assert_eq!(offered(&host), ["agent_roster", "message_agent"]);
    host.attach_agent_peers(std::sync::Arc::new(NoOneElse));
    assert_eq!(
        offered(&host).len(),
        2,
        "a later node replaces the attachment instead of adding to it"
    );

    // The owner's switch for `renoa.agents` governs both tools.
    let plugins = crate::plugins::host::HostPlugins::new(
        host.config.database.clone(),
        agent,
        super::peer_bindings(&host.config, agent, None),
    )
    .expect("Host plugins");
    let found = |plugins: &crate::plugins::host::HostPlugins| {
        plugins
            .matches(None, "agent", false)
            .expect("search")
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(found(&plugins), ["agent_roster", "message_agent"]);
    crate::plugins::host::state::change(
        &host.config.database,
        agent,
        crate::plugins::host::HostPluginId::Agents,
        false,
        "switch-off",
    )
    .expect("turn the Agents plugin off");
    assert!(found(&plugins).is_empty());
}
