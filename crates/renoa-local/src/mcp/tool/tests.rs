use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use serde_json::json;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{EXECUTE_TOOL, LOAD_REFERENCE_LIMIT, LOAD_TOOL, LoadTool, parse_references};
use crate::AgentId;
use crate::mcp::{
    AdapterCatalog, MCP_ADAPTER_REVISION, MCP_PROTOCOL_VERSION, McpCatalogSnapshot,
    McpCatalogStore, McpCatalogTool,
};

#[test]
fn registry_tool_names_are_small_and_stable() {
    assert_eq!([LOAD_TOOL, EXECUTE_TOOL], ["tool_load", "tool_execute"]);
}

#[test]
fn schema_loading_rejects_duplicate_and_oversized_batches() {
    let reference = format!("mcp:github:{}:search_code", "a".repeat(64));
    assert!(parse_references(vec![reference.clone(), reference]).is_err());
    assert!(
        parse_references(
            (0..=LOAD_REFERENCE_LIMIT)
                .map(|index| format!("mcp:github:{}:tool{index}", "a".repeat(64)))
                .collect(),
        )
        .is_err()
    );
}

#[tokio::test]
async fn schema_loading_fails_instead_of_truncating_an_exact_large_schema() {
    let directory = tempdir().expect("temporary Host catalog");
    let store = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("initialize Host catalog");
    store
        .register_direct_connection("fixture", "primary", "http://127.0.0.1:43127/mcp")
        .expect("register connection");
    let schema = json!({"type": "object", "description": "x".repeat(70_000)});
    let snapshot = McpCatalogSnapshot::from_adapter(
        "primary",
        AdapterCatalog {
            endpoint: "http://127.0.0.1:43127/mcp".to_owned(),
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            adapter_revision: MCP_ADAPTER_REVISION.to_owned(),
            tools: vec![McpCatalogTool {
                name: "large".to_owned(),
                description: "Large exact schema".to_owned(),
                input_schema: schema.clone(),
                model_input_schema: schema,
                output_schema: None,
            }],
            rejected_tools: Vec::new(),
        },
    )
    .expect("build catalog");
    let reference = format!("mcp:primary:{}:large", snapshot.digest());
    store.publish_catalog(&snapshot).expect("publish catalog");
    crate::test_agents::insert_agent(store.path(), &agent(1).to_string());
    store
        .enable_agent_connection(&agent(1).to_string(), "primary")
        .expect("enable connection");
    let load = LoadTool::new(agent(1), store);

    let result = invoke_tool(
        Some(&load),
        ToolCall {
            id: "load-large".to_owned(),
            name: LOAD_TOOL.to_owned(),
            arguments: json!({"references": [reference]}),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("schema load has a definite outcome");

    assert!(result.is_error);
    let ContentBlock::Text { text } = &result.content[0] else {
        panic!("schema-load error must be text")
    };
    assert!(text.contains("65536"));
}

fn agent(seed: u128) -> AgentId {
    crate::derived_agent_id(uuid::Uuid::from_u128(seed))
}
