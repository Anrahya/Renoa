use renoa_agent::{ContentBlock, ToolCall, ToolErrorCode, invoke_tool};
use renoa_kernel::{CommandId, SessionId};
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};
use tokio_util::sync::CancellationToken;

mod credential_setup;
mod output;
mod packages;
mod registry;
mod support;
mod transactional;

use super::{ManageTool, TOOL_NAME};
use crate::plugins::tests::test_skill_store;
use crate::{
    AgentProfileId,
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
            .profile_tool_summaries(crate::ALPHA_PROFILE_ID)
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
    assert_eq!(connection["enabled_for_profile"], true);
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
    assert_eq!(disconnected["enabled_for_profile"], false);
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
            .profile_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read tools after disconnect")
            .is_empty()
    );
    assert!(fixture.mcp.load_catalog(&fixture.connection).is_ok());

    let listed = call(&fixture.tool, json!({"action": "list"})).await;
    let connection = inventory_item(&listed, "connection");
    assert_eq!(connection["catalog_loaded"], true);
    assert_eq!(connection["enabled_for_profile"], false);
    let enabled = call(
        &fixture.tool,
        json!({"action": "enable", "connection": fixture.connection}),
    )
    .await;
    assert_eq!(enabled["status"], "enabled");
    assert_eq!(enabled["catalog_retained"], true);
    assert_eq!(enabled["enabled_for_profile"], true);
    assert_eq!(
        fixture
            .mcp
            .profile_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read tools after re-enable")
            .len(),
        1
    );
}

#[tokio::test]
async fn extension_management_changes_only_its_bound_profile() {
    let fixture = ResearchedMcpFixture::new().await;
    let second_profile =
        AgentProfileId::new("renoa.test.second.v1").expect("valid second profile id");
    let second_tool = ManageTool::for_session(
        second_profile.clone(),
        fixture.manager.clone(),
        fixture.directory.path().to_path_buf(),
        SessionId::new(),
        Some(CommandId::new()),
    );

    let before = call(&second_tool, json!({"action": "list"})).await;
    assert_eq!(
        inventory_item(&before, "connection")["enabled_for_profile"],
        false
    );
    call(
        &second_tool,
        json!({"action": "enable", "connection": fixture.connection}),
    )
    .await;
    assert_eq!(
        fixture
            .mcp
            .profile_tool_summaries(second_profile.as_str())
            .expect("read second profile registry")
            .len(),
        1
    );
    call(
        &second_tool,
        json!({"action": "disconnect", "connection": fixture.connection}),
    )
    .await;
    assert!(
        fixture
            .mcp
            .profile_tool_summaries(second_profile.as_str())
            .expect("read second profile after disconnect")
            .is_empty()
    );
    assert_eq!(
        fixture
            .mcp
            .profile_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("Alpha attachment remains unchanged")
            .len(),
        1
    );
}

struct ResearchedMcpFixture {
    directory: TempDir,
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
            directory,
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
