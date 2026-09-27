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
