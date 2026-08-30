use std::fs;

use renoa_agent::{ContentBlock, ToolCall, ToolErrorCode, invoke_tool};
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};
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

fn inventory_items(page: &Value) -> &[Value] {
    page["items"]
        .as_array()
        .map(Vec::as_slice)
        .expect("extension list has inventory items")
}

fn inventory_item<'a>(page: &'a Value, kind: &str) -> &'a Value {
    inventory_items(page)
        .iter()
        .find(|item| item["kind"] == kind)
        .unwrap_or_else(|| panic!("extension list has a {kind} item"))
}

#[tokio::test]
async fn an_agent_researched_mcp_uses_the_same_install_and_hot_load_path() {
    let fixture = ResearchedMcpFixture::new().await;
    let installed = fixture
        .manager
        .list()
        .await
        .expect("list researched package");
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].metadata().name(), "exa-research");
    assert_eq!(
        installed[0].metadata().homepage(),
        Some("https://docs.exa.ai/reference/exa-mcp")
    );
    assert_eq!(
        fixture
            .mcp
            .alpha_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read hot-loaded researched tools")
            .len(),
        1
    );
}

#[tokio::test]
async fn extension_inventory_is_bounded_and_complete() {
    let fixture = ResearchedMcpFixture::new().await;
    let invalid_limit = fixture
        .tool
        .list(None, super::inventory::MAX_LIST_LIMIT + 1)
        .await
        .expect_err("the runtime must enforce the schema's page bound");
    assert_eq!(invalid_limit.code(), ToolErrorCode::InvalidInput);

    let listed = call(&fixture.tool, json!({"action": "list", "limit": 2})).await;
    assert_eq!(listed["returned"], 2);
    assert_eq!(listed["total"], 3);
    let cursor = listed["next_cursor"]
        .as_str()
        .expect("a partial inventory page has a cursor");
    assert_eq!(inventory_items(&listed)[0]["kind"], "package");
    assert_eq!(inventory_items(&listed)[1]["kind"], "package_mcp_server");

    let listed = call(
        &fixture.tool,
        json!({"action": "list", "cursor": cursor, "limit": 2}),
    )
    .await;
    assert_eq!(listed["returned"], 1);
    assert!(listed.get("next_cursor").is_none());
    let connection = inventory_item(&listed, "connection");
    assert_eq!(connection["connection"], fixture.connection);
    assert_eq!(connection["registered"], true);
    assert_eq!(connection["catalog_loaded"], true);
    assert_eq!(connection["enabled_for_alpha"], true);
}

#[tokio::test]
async fn disconnect_and_enable_preserve_one_complete_catalog() {
    let fixture = ResearchedMcpFixture::new().await;
    let listed = call(&fixture.tool, json!({"action": "list", "limit": 2})).await;
    let cursor = listed["next_cursor"]
        .as_str()
        .expect("a partial inventory page has a cursor")
        .to_owned();

    let disconnected = call(
        &fixture.tool,
        json!({"action": "disconnect", "connection": fixture.connection}),
    )
    .await;
    assert_eq!(disconnected["status"], "disconnected");
    assert_eq!(disconnected["catalog_retained"], true);
    assert_eq!(disconnected["enabled_for_alpha"], false);
    let repeated = call(
        &fixture.tool,
        json!({"action": "disconnect", "connection": fixture.connection}),
    )
    .await;
    assert_eq!(repeated, disconnected);
    let stale_cursor = fixture
        .tool
        .list(Some(&cursor), 2)
        .await
        .expect_err("a changed inventory must invalidate its prior cursor");
    assert_eq!(stale_cursor.code(), ToolErrorCode::Conflict);
    assert!(
        fixture
            .mcp
            .alpha_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read tools after disconnect")
            .is_empty()
    );
    assert!(fixture.mcp.load_catalog(&fixture.connection).is_ok());

    let listed = call(&fixture.tool, json!({"action": "list"})).await;
    let connection = inventory_item(&listed, "connection");
    assert_eq!(connection["catalog_loaded"], true);
    assert_eq!(connection["enabled_for_alpha"], false);
    let enabled = call(
        &fixture.tool,
        json!({"action": "enable", "connection": fixture.connection}),
    )
    .await;
    assert_eq!(enabled["status"], "enabled");
    assert_eq!(enabled["catalog_retained"], true);
    assert_eq!(enabled["enabled_for_alpha"], true);
    assert_eq!(
        fixture
            .mcp
            .alpha_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read tools after re-enable")
            .len(),
        1
    );
}

struct ResearchedMcpFixture {
    _directory: TempDir,
    mcp: McpCatalogStore,
    manager: PluginManager,
    tool: ManageTool,
    connection: String,
}

impl ResearchedMcpFixture {
    async fn new() -> Self {
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
            skills,
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
        Self {
            _directory: directory,
            mcp,
            manager,
            tool,
            connection,
        }
    }
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
  wire_version: 7,
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
