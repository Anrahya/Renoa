use serde_json::Value;
use tempfile::tempdir;

use super::PluginSearchTool;
use crate::{
    host::catalog,
    mcp::{McpCatalogStore, McpCredentialResolver, tests::snapshot},
    plugins::PluginManager,
    skills::SkillStore,
};

#[tokio::test]
async fn targeted_search_keeps_valid_previews_when_inventory_references_go_stale() {
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
    let agent = crate::derived_agent_id(uuid::Uuid::from_u128(8));
    let search = PluginSearchTool::new(agent, manager, false);
    crate::test_agents::insert_agent(mcp.path(), &agent.to_string());

    for connection in ["stale", "valid"] {
        mcp.register_direct_connection(connection, connection, "http://127.0.0.1:43127/mcp")
            .expect("register connection");
        publish(&mcp, connection, "find_item");
        mcp.enable_agent_connection(&agent.to_string(), connection)
            .expect("enable connection");
    }
    let inventory = search.local_inventory().await.expect("initial inventory");
    publish(&mcp, "stale", "replacement");

    let cards = search
        .search_cards(&inventory, "find_item", 0)
        .await
        .expect("search should survive one refreshed catalog");
    let cards = decode(&cards);
    assert_eq!(cards["total"], 2);
    assert_eq!(cards["tool_matches"].as_array().unwrap().len(), 1);
    assert_eq!(cards["tool_matches"][0]["name"], "find_item");
    assert!(cards["tool_matches"][0]["input_schema"].is_object());

    let connection = search
        .search_connection(&inventory, "stale", "find_item", 0)
        .await
        .expect("connection page should survive a refreshed catalog");
    let connection = decode(&connection);
    assert_eq!(connection["total"], 1);
    assert_eq!(connection["items"][0]["name"], "find_item");
    assert!(connection["items"][0].get("input_schema").is_none());

    mcp.disable_agent_connection(&agent.to_string(), "valid")
        .expect("disable connection");
    let cards = search
        .search_cards(&inventory, "find_item", 0)
        .await
        .expect("search should survive disabled preview references");
    assert_eq!(decode(&cards)["total"], 2);
}

fn publish(mcp: &McpCatalogStore, connection: &str, name: &str) {
    let snapshot = snapshot(connection, "http://127.0.0.1:43127/mcp", &[name]);
    mcp.publish_catalog(&snapshot).expect("publish catalog");
}

fn decode(output: &renoa_agent::ToolOutput) -> Value {
    let [renoa_agent::ContentBlock::Text { text }] = output.content.as_slice() else {
        panic!("one JSON search result")
    };
    serde_json::from_str(text).expect("decode search output")
}
