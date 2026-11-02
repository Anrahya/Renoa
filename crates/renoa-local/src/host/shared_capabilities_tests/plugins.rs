use super::*;

#[tokio::test]
async fn disabled_retained_plugin_does_not_block_selecting_another_plugin() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    prepare_fixture(root);
    let host = host(root);
    let agent = create_agent(&host, "Lifecycle").await;
    let workspace = root.join("workspace");
    for (name, connection) in [("first-service", "first"), ("second-service", "second")] {
        host.register_gh_cli_mcp_connection(
            name,
            connection,
            "https://example.com/mcp",
            "github.com",
            "fixture-owner",
        )
        .await
        .unwrap();
        host.refresh_mcp_catalog(connection).await.unwrap();
    }
    host.enable_agent_connection(agent, "first").await.unwrap();
    let first = host
        .installed_plugins()
        .await
        .unwrap()
        .into_iter()
        .find(|plugin| plugin.metadata().name() == "first-service")
        .unwrap();
    host.manage_plugin(
        &agent,
        &workspace,
        PluginRequest::Deactivate {
            plugin_id: first.digest().to_owned(),
        },
        PluginInvocation {
            operation_id: "disable-first",
            updates: None,
            cancellation: CancellationToken::new(),
        },
    )
    .await
    .unwrap();
    host.enable_agent_connection(agent, "second")
        .await
        .expect("retained inactive first selection must not block another plugin");
    assert!(host.enable_agent_connection(agent, "first").await.is_err());
    let visible = host
        .config
        .mcp_catalog
        .agent_tool_summaries(&agent.to_string())
        .unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].reference().unwrap().connection_id(), "second");
    let selections = host
        .agent_definition(agent)
        .await
        .unwrap()
        .unwrap()
        .connections;
    assert_eq!(
        selections,
        BTreeSet::from(["first".to_owned(), "second".to_owned()])
    );
    host.manage_plugin(
        &agent,
        &workspace,
        PluginRequest::EnablePlugin {
            plugin_id: first.digest().to_owned(),
        },
        PluginInvocation {
            operation_id: "restore-first",
            updates: None,
            cancellation: CancellationToken::new(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        host.config
            .mcp_catalog
            .agent_tool_summaries(&agent.to_string())
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        fs::read_to_string(root.join("mcp-calls"))
            .unwrap()
            .lines()
            .count(),
        2,
        "lifecycle toggles use the retained catalogs without new network calls"
    );
}

#[tokio::test]
async fn an_ability_chosen_for_an_agent_is_not_reported_and_one_it_turns_on_is() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    prepare_fixture(root);
    let host = host(root);
    for (name, connection) in [("first-service", "first"), ("second-service", "second")] {
        host.register_gh_cli_mcp_connection(
            name,
            connection,
            "https://example.com/mcp",
            "github.com",
            "fixture-owner",
        )
        .await
        .unwrap();
        host.refresh_mcp_catalog(connection).await.unwrap();
    }
    // Its creator chose `first` at creation, and the owner adds `second`;
    // each selection creates the plugin's activation.
    let agent = host
        .create_agent(
            AgentCreator::System {
                component: "shared-capabilities".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(crate::presets::ALPHA_PRESET_ID).expect("preset"),
                "Chosen for",
            )
            .with_connections(["first".to_owned()]),
            CancellationToken::new(),
        )
        .await
        .expect("agent")
        .id;
    host.enable_agent_connection(agent, "second").await.unwrap();
    let feed = crate::HostFeed::open(host.config.home.path()).unwrap();
    assert!(feed.list(None).await.unwrap().items.is_empty());

    let second = host
        .installed_plugins()
        .await
        .unwrap()
        .into_iter()
        .find(|plugin| plugin.metadata().name() == "second-service")
        .unwrap();
    for (operation, request) in [
        (
            "off",
            PluginRequest::Deactivate {
                plugin_id: second.digest().to_owned(),
            },
        ),
        (
            "on",
            PluginRequest::EnablePlugin {
                plugin_id: second.digest().to_owned(),
            },
        ),
        (
            "on",
            PluginRequest::EnablePlugin {
                plugin_id: second.digest().to_owned(),
            },
        ),
    ] {
        host.manage_plugin(
            &agent,
            &root.join("workspace"),
            request,
            PluginInvocation {
                operation_id: operation,
                updates: None,
                cancellation: CancellationToken::new(),
            },
        )
        .await
        .unwrap();
    }
    let items = feed.list(None).await.unwrap().items;
    assert_eq!(
        items
            .iter()
            .map(|item| (item.agent.id, item.title.as_str(), item.subject_id.as_str()))
            .collect::<Vec<_>>(),
        [(agent, "Turned on second-service", second.digest())],
        "the agent's own turn-on is reported once"
    );
}
