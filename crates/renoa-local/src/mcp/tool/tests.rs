use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{
    EXECUTE_TOOL, LOAD_REFERENCE_LIMIT, LOAD_TOOL, LoadTool, SEARCH_RESULT_LIMIT, SEARCH_TOOL,
    SearchTool, parse_references,
};
use crate::mcp::{
    AdapterCatalog, MCP_ADAPTER_REVISION, MCP_PROTOCOL_VERSION, McpCatalogSnapshot,
    McpCatalogStore, McpCatalogTool,
};
use crate::{ALPHA_PROFILE_ID, AgentProfileId};

#[test]
fn registry_tool_names_are_small_and_stable() {
    assert_eq!(
        [SEARCH_TOOL, LOAD_TOOL, EXECUTE_TOOL],
        ["tool_search", "tool_load", "tool_execute",]
    );
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
async fn one_live_registry_tool_sees_a_thousand_new_tools_without_a_schema_dump() {
    let directory = tempdir().expect("temporary Host catalog");
    let store = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("initialize Host catalog");
    let search_tool = SearchTool::new(alpha_id(), store.clone());
    let before = run_search(&search_tool, "tool").await;
    assert_eq!(before["total_matches"], 0);

    store
        .register_direct_connection("fixture", "primary", "http://127.0.0.1:43127/mcp")
        .expect("register connection after constructing registry tool");
    let tools = (0..1_000)
        .map(|index| McpCatalogTool {
            name: format!("tool_{index:04}"),
            description: format!("Fixture capability {index}"),
            input_schema: json!({
                "type": "object",
                "properties": {"value": {"type": "string"}}
            }),
            model_input_schema: json!({
                "type": "object",
                "properties": {"value": {"type": "string"}}
            }),
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
    .expect("build large catalog");
    store
        .publish_catalog(&snapshot)
        .expect("publish large catalog");
    store
        .enable_profile_connection(ALPHA_PROFILE_ID, "primary")
        .expect("enable connection");

    let after = run_search(&search_tool, "tool").await;
    assert_eq!(after["total_matches"], 1_000);
    assert_eq!(
        after["matches"]
            .as_array()
            .expect("search matches array")
            .len(),
        SEARCH_RESULT_LIMIT
    );
    let first = after["matches"][0]
        .as_object()
        .expect("compact search match object");
    let mut keys = first.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, ["description", "name", "reference"]);
    assert!(!after.to_string().contains("input_schema"));
}

#[tokio::test]
async fn live_registry_tools_read_only_their_profile_attachments() {
    let directory = tempdir().expect("temporary Host catalog");
    let store = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("initialize Host catalog");
    let second = AgentProfileId::new("renoa.test.second.v1").expect("valid second profile id");
    let alpha_search = SearchTool::new(alpha_id(), store.clone());
    let second_search = SearchTool::new(second.clone(), store.clone());
    store
        .register_direct_connection("fixture", "primary", "http://127.0.0.1:43127/mcp")
        .expect("register connection");
    let snapshot = McpCatalogSnapshot::from_adapter(
        "primary",
        AdapterCatalog {
            endpoint: "http://127.0.0.1:43127/mcp".to_owned(),
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            adapter_revision: MCP_ADAPTER_REVISION.to_owned(),
            tools: vec![McpCatalogTool {
                name: "echo".to_owned(),
                description: "Echo one value".to_owned(),
                input_schema: json!({"type": "object"}),
                model_input_schema: json!({"type": "object"}),
                output_schema: None,
            }],
            rejected_tools: Vec::new(),
        },
    )
    .expect("build catalog");
    store.publish_catalog(&snapshot).expect("publish catalog");
    store
        .enable_profile_connection(ALPHA_PROFILE_ID, "primary")
        .expect("attach catalog to Alpha");

    assert_eq!(run_search(&alpha_search, "echo").await["total_matches"], 1);
    assert_eq!(run_search(&second_search, "echo").await["total_matches"], 0);

    store
        .enable_profile_connection(second.as_str(), "primary")
        .expect("share catalog with second profile");
    assert_eq!(run_search(&second_search, "echo").await["total_matches"], 1);
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
    store
        .enable_profile_connection(ALPHA_PROFILE_ID, "primary")
        .expect("enable connection");
    let load = LoadTool::new(alpha_id(), store);

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

fn alpha_id() -> AgentProfileId {
    AgentProfileId::new(ALPHA_PROFILE_ID).expect("valid Alpha profile id")
}

async fn run_search(tool: &SearchTool, query: &str) -> Value {
    let result = invoke_tool(
        Some(tool),
        ToolCall {
            id: "search-fixture".to_owned(),
            name: SEARCH_TOOL.to_owned(),
            arguments: json!({"query": query}),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("search has a definite outcome");
    assert!(!result.is_error, "search failed: {result:?}");
    let ContentBlock::Text { text } = &result.content[0] else {
        panic!("search result must be text")
    };
    serde_json::from_str(text).expect("search returns JSON")
}
