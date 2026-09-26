use super::*;

#[tokio::test]
async fn oauth_replacement_does_not_reuse_the_previous_endpoints_auth_reference() {
    use crate::mcp::{McpConnectionAuth, McpOAuthRegistration, McpRequestHeaders};

    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    let previous_endpoint = "https://example.com/previous";
    let previous_auth = McpConnectionAuth::oauth(
        "account",
        previous_endpoint,
        McpOAuthRegistration::dynamic(),
    )
    .expect("previous endpoint-bound reference");
    manager
        .mcp_catalog()
        .register_connection(
            "previous",
            "account",
            previous_endpoint,
            &McpRequestHeaders::default(),
            &previous_auth,
        )
        .expect("existing OAuth connection");
    let request = |replace| PluginRequest::Add {
        source: PluginSource::Mcp {
            name: "replacement".to_owned(),
            description: "Replacement service.".to_owned(),
            server: "api".to_owned(),
            endpoint: "https://example.com/replacement".to_owned(),
            documentation: "https://example.com/docs".to_owned(),
            headers: Vec::new(),
        },
        expected_digest: None,
        server: None,
        connection: Some("account".to_owned()),
        credential: Some(PluginAuthentication::OAuth {}),
        replace,
    };
    assert!(matches!(
        invoke(&manager, root.path(), request(false)).await,
        Err(PluginError::Mcp(_))
    ));
    assert!(
        manager
            .list()
            .await
            .expect("no replacement admitted")
            .is_empty()
    );
    let PluginOutcome::Added(added) = invoke(&manager, root.path(), request(true))
        .await
        .expect("explicit replacement passes static validation")
    else {
        panic!("replacement admission")
    };
    assert_eq!(added.installed.metadata().name(), "replacement");
    assert!(matches!(
        added.connection,
        crate::PluginConnectionOutcome::Failed {
            error: PluginError::Unavailable(_),
            ..
        }
    ));
    let previous = manager
        .mcp_catalog()
        .connection_config("account")
        .expect("unavailable discovery retains the previous connection");
    assert_eq!(previous.endpoint, previous_endpoint);
    assert_eq!(previous.auth, previous_auth);
    assert_eq!(
        manager.list().await.expect("one retained revision").len(),
        1
    );
}

#[tokio::test]
async fn a_known_connection_conflict_leaves_no_plugin_or_skills() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, skills) = manager(root.path());
    let source = root.path().join("package");
    write_skill(&source.join("skills/review"));
    fs::write(
        source.join("plugin.json"),
        serde_json::json!({"$schema":crate::plugins::inspect::PLUGIN_SCHEMA,"name":"package"})
            .to_string(),
    )
    .expect("plugin manifest");
    fs::write(source.join("mcp.json"),serde_json::json!({"$schema":crate::plugins::inspect::MCP_SCHEMA,"mcpServers":{"api":{"type":"streamable-http","url":"https://example.com/new"}}}).to_string()).expect("server manifest");
    manager
        .mcp_catalog()
        .register_direct_connection("prior", "account", "https://example.com/prior")
        .expect("existing connection");
    let source = PluginSource::Package {
        source_path: "package".into(),
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    assert!(matches!(
        invoke(
            &manager,
            root.path(),
            PluginRequest::Add {
                source,
                expected_digest: Some(digest),
                server: Some("api".to_owned()),
                connection: Some("account".to_owned()),
                credential: None,
                replace: false
            }
        )
        .await,
        Err(PluginError::Mcp(crate::McpHostError::Conflict(_)))
    ));
    assert!(manager.list().await.expect("no installation").is_empty());
    assert_eq!(
        fs::read_dir(root.path().join("plugins"))
            .expect("no publication")
            .count(),
        0
    );
    assert!(
        skills
            .summaries(&test_agent_id(1).to_string(), root.path())
            .expect("no skill binding")
            .is_empty()
    );
    assert_eq!(
        manager
            .mcp_catalog()
            .connection_endpoint("account")
            .expect("original connection"),
        "https://example.com/prior"
    );
}

#[tokio::test]
async fn typed_inventory_pages_cover_exactly_the_requested_facts() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    write_skill(&root.path().join("skill"));
    let source = PluginSource::Skill {
        source_path: "skill".into(),
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    invoke(
        &manager,
        root.path(),
        PluginRequest::Add {
            source,
            expected_digest: Some(digest.clone()),
            server: None,
            connection: None,
            credential: None,
            replace: false,
        },
    )
    .await
    .expect("add");
    let mut cursor = None;
    let mut total = 0;
    let mut returned = 0;
    let mut names = Vec::new();
    loop {
        let PluginOutcome::Listed(page) = invoke(
            &manager,
            root.path(),
            PluginRequest::List { cursor, limit: 1 },
        )
        .await
        .expect("typed page") else {
            panic!("list")
        };
        assert_eq!(page.returned(), 1);
        assert_eq!(page.items().len(), 1);
        total = total.max(page.total());
        returned += page.returned();
        for item in page.items() {
            match item {
                PluginInventoryItem::Package {
                    package_digest,
                    name,
                    ..
                } => {
                    assert_eq!(package_digest, &digest);
                    names.push(name.clone());
                }
                PluginInventoryItem::PluginSkill { name, .. } => names.push(name.clone()),
                _ => (),
            }
        }
        cursor = page.next_cursor().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(total, 3);
    assert_eq!(returned, total);
    assert_eq!(names, ["review", "review"]);
}
