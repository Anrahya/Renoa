use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use crate::plugins::PluginSearchTool;
use crate::{
    host::catalog,
    mcp::{
        AdapterCatalog, MCP_ADAPTER_REVISION, MCP_PROTOCOL_VERSION, McpCatalogSnapshot,
        McpCatalogStore, McpCatalogTool, McpCredentialResolver,
    },
    output::MAX_TOOL_OUTPUT_BYTES,
    plugins::PluginManager,
    skills::SkillStore,
};

#[tokio::test]
async fn local_search_groups_many_mcp_tools_under_one_plugin_and_pages_nested_tools() {
    let directory = tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let manager = PluginManager::initialize(
        database.clone(),
        directory.path().join("plugins"),
        mcp.clone(),
        None,
        None,
        McpCredentialResolver::default(),
        SkillStore::initialize(database.clone(), directory.path().join("skills"), None)
            .expect("initialize skill store"),
    )
    .expect("initialize plugin manager");
    let agent = crate::derived_agent_id(uuid::Uuid::from_u128(1));
    let search = PluginSearchTool::new(agent, manager.clone(), false);
    let other_agent = crate::derived_agent_id(uuid::Uuid::from_u128(2));
    let other = PluginSearchTool::new(other_agent, manager.clone(), false);
    let other_manager = PluginSearchTool::new(other_agent, manager, true);
    assert_eq!(call(&search, json!({"query":"*"})).await["total"], 0);

    mcp.register_direct_connection("fixture", "primary", "http://127.0.0.1:43127/mcp")
        .expect("register connection");
    let tools = (0..1_000)
        .map(|index| McpCatalogTool {
            name: format!("tool_{index:04}"),
            description: format!("Fixture capability {index} {}", "🦊".repeat(320)),
            input_schema: json!({"type":"object", "properties":{"value":{"type":"string"}}}),
            model_input_schema: json!({"type":"object", "properties":{"value":{"type":"string"}}}),
            output_schema: None,
        })
        .collect();
    let snapshot = McpCatalogSnapshot::from_adapter(
        "primary",
        AdapterCatalog {
            endpoint: "http://127.0.0.1:43127/mcp".to_owned(),
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            adapter_revision: MCP_ADAPTER_REVISION.to_owned(),
            tools,
            rejected_tools: Vec::new(),
        },
    )
    .expect("build large MCP catalog");
    mcp.publish_catalog(&snapshot).expect("publish catalog");
    crate::test_agents::insert_agent(mcp.path(), &agent.to_string());
    mcp.enable_agent_connection(&agent.to_string(), "primary")
        .expect("enable for one agent");

    let local = call(&search, json!({"query":"tool_0999"})).await;
    assert_eq!(local["total"], 1);
    assert_eq!(local["items"][0]["id"], "direct:fixture");
    assert_eq!(local["items"][0]["catalog_tool_count"], 1_000);
    assert_eq!(local["items"][0]["credential_configured_connections"], 0);
    assert_eq!(local["tool_matches"][0]["name"], "tool_0999");
    assert_eq!(
        local["tool_matches"][0]["input_schema"]["properties"]["value"]["type"],
        "string"
    );
    let reference = local["tool_matches"][0]["reference"]
        .as_str()
        .expect("exact tool reference");
    let exact = call(&search, json!({"reference": reference})).await;
    assert_eq!(exact["reference"], reference);
    assert_eq!(exact["name"], "tool_0999");
    assert_eq!(
        exact["input_schema"],
        local["tool_matches"][0]["input_schema"]
    );
    let inspected = call(&search, json!({"plugin":"direct:fixture"})).await;
    assert_eq!(inspected["items"][0]["kind"], "connection");
    assert_eq!(inspected["items"][0]["connection"], "primary");
    assert_eq!(inspected["items"][0]["enabled_for_agent"], true);
    assert_eq!(inspected["items"][0]["credential_configured"], false);

    let first = call(&search, json!({"connection":"primary", "query":"*"})).await;
    assert_eq!(first["total"], 1_000);
    let items = first["items"].as_array().expect("nested MCP tools");
    assert!(!items.is_empty() && items.len() <= 200);
    assert_eq!(first["next_offset"], items.len());
    assert!(first.to_string().len() <= MAX_TOOL_OUTPUT_BYTES);
    assert!(!first.to_string().contains("input_schema"));
    assert!(
        items[0]["reference"]
            .as_str()
            .is_some_and(|value| value.starts_with("mcp:primary:"))
    );
    let targeted = call(
        &search,
        json!({"connection":"primary", "query":"tool_0999"}),
    )
    .await;
    assert!(targeted["total"].as_u64().is_some_and(|count| count >= 1));
    assert_eq!(targeted["items"][0]["name"], "tool_0999");
    assert_eq!(
        targeted["items"][0]["input_schema"]["properties"]["value"]["type"],
        "string"
    );

    assert_visibility_and_stale_references(&mcp, &search, &other, &other_manager, reference).await;
}

async fn assert_visibility_and_stale_references(
    mcp: &McpCatalogStore,
    search: &PluginSearchTool,
    other: &PluginSearchTool,
    other_manager: &PluginSearchTool,
    reference: &str,
) {
    let hidden = call(other, json!({"query":"fixture"})).await;
    assert_eq!(
        hidden["total"], 0,
        "an agent without management access cannot inspect another agent's connection"
    );
    let other_card = call(other_manager, json!({"query":"fixture"})).await;
    assert_eq!(other_card["items"][0]["enabled_connections"], 0);
    let refused = invoke_tool(
        Some(other),
        ToolCall {
            id: "disabled-tools".to_owned(),
            name: crate::capabilities::PLUGIN_SEARCH.to_owned(),
            arguments: json!({"connection":"primary"}),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("disabled connection has a definite result");
    assert!(refused.is_error);
    let refused_exact = invoke_tool(
        Some(other),
        ToolCall {
            id: "disabled-reference".to_owned(),
            name: crate::capabilities::PLUGIN_SEARCH.to_owned(),
            arguments: json!({"reference": reference}),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("disabled reference has a definite result");
    assert!(refused_exact.is_error);

    let mixed = invoke_tool(
        Some(search),
        ToolCall {
            id: "mixed-reference".to_owned(),
            name: crate::capabilities::PLUGIN_SEARCH.to_owned(),
            arguments: json!({"reference": reference, "query": "tool_0999"}),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("mixed selectors have a definite result");
    assert!(mixed.is_error);

    let refreshed = McpCatalogSnapshot::from_adapter(
        "primary",
        AdapterCatalog {
            endpoint: "http://127.0.0.1:43127/mcp".to_owned(),
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            adapter_revision: MCP_ADAPTER_REVISION.to_owned(),
            tools: vec![McpCatalogTool {
                name: "replacement".to_owned(),
                description: "New catalog".to_owned(),
                input_schema: json!({"type":"object"}),
                model_input_schema: json!({"type":"object"}),
                output_schema: None,
            }],
            rejected_tools: Vec::new(),
        },
    )
    .expect("refresh catalog");
    mcp.publish_catalog(&refreshed)
        .expect("publish refreshed catalog");
    let stale = invoke_tool(
        Some(search),
        ToolCall {
            id: "stale-reference".to_owned(),
            name: crate::capabilities::PLUGIN_SEARCH.to_owned(),
            arguments: json!({"reference": reference}),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("stale reference has a definite result");
    assert!(stale.is_error);
    let ContentBlock::Text { text } = &stale.content[0] else {
        panic!("stale reference error must be text")
    };
    assert!(text.contains("stale"));
}

#[tokio::test]
async fn exact_search_returns_complete_large_schema_or_fails_without_truncation() {
    let directory = tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let manager = PluginManager::initialize(
        database.clone(),
        directory.path().join("plugins"),
        mcp.clone(),
        None,
        None,
        McpCredentialResolver::default(),
        SkillStore::initialize(database, directory.path().join("skills"), None)
            .expect("initialize skill store"),
    )
    .expect("initialize plugin manager");
    let agent = crate::derived_agent_id(uuid::Uuid::from_u128(3));
    let search = PluginSearchTool::new(agent, manager, false);
    mcp.register_direct_connection("fixture", "primary", "http://127.0.0.1:43127/mcp")
        .expect("register connection");
    crate::test_agents::insert_agent(mcp.path(), &agent.to_string());

    for schema_size in [12_000, 70_000] {
        let schema = json!({"type":"object", "description":"x".repeat(schema_size)});
        let snapshot = McpCatalogSnapshot::from_adapter(
            "primary",
            AdapterCatalog {
                endpoint: "http://127.0.0.1:43127/mcp".to_owned(),
                protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
                adapter_revision: MCP_ADAPTER_REVISION.to_owned(),
                tools: vec![McpCatalogTool {
                    name: "large".to_owned(),
                    description: "Large schema".to_owned(),
                    input_schema: schema.clone(),
                    model_input_schema: schema.clone(),
                    output_schema: None,
                }],
                rejected_tools: Vec::new(),
            },
        )
        .expect("build catalog");
        mcp.publish_catalog(&snapshot).expect("publish catalog");
        if schema_size == 12_000 {
            mcp.enable_agent_connection(&agent.to_string(), "primary")
                .expect("enable connection");
        }
        let preview = call(&search, json!({"query":"large"})).await;
        let reference = preview["tool_matches"][0]["reference"]
            .as_str()
            .expect("reference");
        assert!(preview["tool_matches"][0].get("input_schema").is_none());
        let result = invoke_tool(
            Some(&search),
            ToolCall {
                id: "large-schema".to_owned(),
                name: crate::capabilities::PLUGIN_SEARCH.to_owned(),
                arguments: json!({"reference":reference}),
                thought_signature: None,
                namespace: None,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .expect("definite result");
        if schema_size == 12_000 {
            assert!(!result.is_error);
            let ContentBlock::Text { text } = &result.content[0] else {
                panic!("text result")
            };
            let exact: Value = serde_json::from_str(text).expect("exact schema JSON");
            assert_eq!(exact["input_schema"], schema);
        } else {
            assert!(result.is_error);
            let ContentBlock::Text { text } = &result.content[0] else {
                panic!("text error")
            };
            assert!(text.contains("65536"));
        }
    }
}

async fn call(tool: &PluginSearchTool, arguments: Value) -> Value {
    let result = invoke_tool(
        Some(tool),
        ToolCall {
            id: "plugin-search-fixture".to_owned(),
            name: crate::capabilities::PLUGIN_SEARCH.to_owned(),
            arguments,
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("plugin search has a definite result");
    assert!(!result.is_error, "plugin search failed: {result:?}");
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("one JSON result")
    };
    serde_json::from_str(text).expect("decode plugin search result")
}
