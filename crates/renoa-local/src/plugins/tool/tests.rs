use std::fs;

use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

mod output;
mod packages;
mod registry;
mod support;

use super::{ManageTool, TOOL_NAME};
use crate::plugins::tests::test_skill_store;
use crate::{
    host::catalog,
    mcp::{McpCatalogStore, McpCredentialResolver},
    plugins::PluginManager,
};
use support::write_mcp_adapter;

#[tokio::test]
async fn an_agent_researched_mcp_uses_the_same_install_and_hot_load_path() {
    let directory = tempdir().expect("temporary researched MCP fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let mcp_adapter = directory.path().join("mcp.mjs");
    write_mcp_adapter(&mcp_adapter);
    let skills = test_skill_store(&database, directory.path());
    let manager = PluginManager::initialize(
        database,
        directory.path().join("installed"),
        mcp.clone(),
        Some(mcp_adapter),
        None,
        McpCredentialResolver::default(),
        skills.clone(),
    )
    .expect("initialize extension manager");
    let tool = ManageTool::new(manager.clone(), directory.path().to_path_buf());

    let added = call(
        &tool,
        json!({
            "action": "add",
            "source": {
                "kind": "mcp",
                "name": "exa-research",
                "description": "Web search through the documented Exa MCP endpoint.",
                "server": "exa",
                "endpoint": "https://mcp.exa.ai/mcp",
                "documentation": "https://docs.exa.ai/reference/exa-mcp"
            }
        }),
    )
    .await;

    assert_eq!(added["status"], "catalog_loaded");
    assert_eq!(added["source"], "mcp");
    assert_eq!(added["tools"], 1);
    let connection = added["connection"]
        .as_str()
        .expect("generated connection id")
        .to_owned();
    let installed = manager.list().await.expect("list researched package");
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].metadata().name(), "exa-research");
    assert_eq!(
        installed[0].metadata().homepage(),
        Some("https://docs.exa.ai/reference/exa-mcp")
    );
    assert_eq!(
        mcp.alpha_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read hot-loaded researched tools")
            .len(),
        1
    );

    let listed = call(&tool, json!({"action": "list"})).await;
    assert_eq!(listed["installed"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["rejected"], json!([]));
    assert_eq!(listed["connections"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["connections"][0]["connection"], connection);
    assert_eq!(listed["connections"][0]["registered"], true);
    assert_eq!(listed["connections"][0]["catalog_loaded"], true);
    assert_eq!(listed["connections"][0]["enabled_for_alpha"], true);

    let disconnected = call(
        &tool,
        json!({"action": "disconnect", "connection": connection}),
    )
    .await;
    assert_eq!(disconnected["status"], "disconnected");
    assert_eq!(disconnected["catalog_retained"], true);
    assert_eq!(disconnected["enabled_for_alpha"], false);
    let repeated = call(
        &tool,
        json!({"action": "disconnect", "connection": connection}),
    )
    .await;
    assert_eq!(repeated, disconnected);
    assert!(
        mcp.alpha_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read tools after disconnect")
            .is_empty()
    );
    assert!(mcp.load_catalog(&connection).is_ok());
    let listed = call(&tool, json!({"action": "list"})).await;
    assert_eq!(listed["connections"][0]["catalog_loaded"], true);
    assert_eq!(listed["connections"][0]["enabled_for_alpha"], false);
}

#[tokio::test]
async fn researched_mcp_public_headers_cannot_smuggle_a_credential_into_a_package() {
    let directory = tempdir().expect("temporary researched MCP header fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let skills = test_skill_store(&database, directory.path());
    let manager = PluginManager::initialize(
        database,
        directory.path().join("installed"),
        mcp,
        None,
        None,
        McpCredentialResolver::default(),
        skills,
    )
    .expect("initialize extension manager");
    let tool = ManageTool::new(manager.clone(), directory.path().to_path_buf());

    let result = invoke_tool(
        Some(&tool),
        ToolCall {
            id: "researched-secret-header".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: json!({
                "action": "add",
                "source": {
                    "kind": "mcp",
                    "name": "unsafe-research",
                    "description": "Unsafe fixture.",
                    "server": "unsafe",
                    "endpoint": "https://example.com/mcp",
                    "documentation": "https://example.com/docs",
                    "headers": [{"name": "Authorization", "value": "Bearer secret"}]
                }
            }),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("secret header rejection is definite");

    assert!(result.is_error);
    assert!(
        manager
            .list()
            .await
            .expect("list packages after rejected source")
            .is_empty()
    );
}

#[tokio::test]
async fn connection_failures_keep_actionable_model_facts_and_host_diagnostics() {
    let directory = tempdir().expect("temporary extension error fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let adapter = directory.path().join("failed-adapter.mjs");
    fs::write(
        &adapter,
        r#"
for await (const _chunk of process.stdin) {}
process.stdout.write(JSON.stringify({
  wire_version: 6,
  event: "failed",
  failure: {
    kind: "incompatible_protocol",
    certainty: "definite",
    message: "The endpoint supports no MCP version Renoa can use.",
    partial_changes_possible: false,
    diagnostic: {
      code: "ERA_NEGOTIATION_FAILED",
      http_status: 400,
      detail: "server offered 2023-01-01"
    }
  }
}) + "\n");
"#,
    )
    .expect("write failed adapter");
    let skills = test_skill_store(&database, directory.path());
    let manager = PluginManager::initialize(
        database,
        directory.path().join("installed"),
        mcp.clone(),
        Some(adapter),
        None,
        McpCredentialResolver::default(),
        skills,
    )
    .expect("initialize extension manager");
    let source = directory.path().join("source");
    fs::create_dir(&source).expect("create plugin source");
    crate::plugins::tests::write_exa_plugin(&source, "https://example.com/mcp");
    let inspection = manager.inspect(&source).await.expect("inspect package");
    manager
        .install(&source, inspection.digest())
        .await
        .expect("install package");
    let tool = ManageTool::new(manager, directory.path().to_path_buf());

    let result = invoke_tool(
        Some(&tool),
        ToolCall {
            id: "connect-error".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: json!({
                "action": "connect",
                "package_digest": inspection.digest(),
                "server": "exa",
                "connection": "exa"
            }),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("discovery failure is definite");

    assert!(result.is_error);
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("extension error must return one text block")
    };
    let model: Value = serde_json::from_str(text).expect("decode model error");
    assert_eq!(model["code"], "mcp_incompatible_protocol");
    assert_eq!(
        model["message"],
        "The endpoint supports no MCP version Renoa can use."
    );
    assert_eq!(model["retryable"], false);
    assert!(
        model["next_action"]
            .as_str()
            .is_some_and(|value| value.contains("Do not retry"))
    );
    let details = result.details.expect("Host keeps the safe diagnostic");
    assert_eq!(
        details["mcp"]["failure"]["diagnostic"]["code"],
        "ERA_NEGOTIATION_FAILED"
    );
    assert_eq!(
        details["mcp"]["failure"]["diagnostic"]["detail"],
        "server offered 2023-01-01"
    );
    assert_failed_connection_is_registered_without_a_catalog(&mcp);
    assert!(
        mcp.alpha_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read Alpha registry")
            .is_empty()
    );
}

fn assert_failed_connection_is_registered_without_a_catalog(mcp: &McpCatalogStore) {
    let connection = mcp
        .connection_config("exa")
        .expect("failed discovery keeps the registered connection configuration");
    assert_eq!(connection.endpoint, "https://example.com/mcp");
    assert!(matches!(
        mcp.load_catalog("exa"),
        Err(crate::mcp::McpHostError::NotFound(_))
    ));
}

async fn call(tool: &ManageTool, arguments: Value) -> Value {
    let result = invoke_tool(
        Some(tool),
        ToolCall {
            id: "extension-call".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments,
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("extension management has a definite result");
    assert!(!result.is_error, "extension management failed: {result:?}");
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("extension management must return one text block")
    };
    serde_json::from_str(text).expect("decode extension management result")
}
