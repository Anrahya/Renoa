use std::fs;

use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

mod output;
mod packages;
mod support;

use super::{ManageTool, TOOL_NAME};
use crate::plugins::tests::test_skill_store;
use crate::{
    host::catalog,
    mcp::{McpCatalogStore, McpCredentialResolver},
    plugins::{IntegrationCatalog, PluginManager},
};
use support::{
    write_catalog_adapter, write_failed_mcp_adapter, write_mcp_adapter,
    write_single_candidate_catalog_adapter,
};

const CATALOG_REFERENCE: &str = "integrations.sh/exa.ai/exa-mcp-server/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[tokio::test]
async fn plain_language_discovery_can_add_and_hot_load_a_public_mcp() {
    let directory = tempdir().expect("temporary catalog extension fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let actions = directory.path().join("catalog-actions");
    let catalog_adapter = directory.path().join("catalog.mjs");
    write_catalog_adapter(&catalog_adapter, &actions);
    let mcp_adapter = directory.path().join("mcp.mjs");
    write_mcp_adapter(&mcp_adapter);
    let skills = test_skill_store(&database, directory.path());
    let manager = PluginManager::initialize(
        database,
        directory.path().join("installed"),
        mcp.clone(),
        Some(mcp_adapter),
        McpCredentialResolver::default(),
        Some(IntegrationCatalog::new(catalog_adapter)),
        skills,
    )
    .expect("initialize extension manager");
    let tool = ManageTool::new(manager.clone(), directory.path().to_path_buf());

    let searched = call(
        &tool,
        json!({"action": "search", "query": "web search with Exa"}),
    )
    .await;
    let reference = searched["candidates"][0]["reference"]
        .as_str()
        .expect("search returned candidate")
        .to_owned();
    let add_request = json!({
        "action": "add",
        "source": {"kind": "catalog", "candidate": reference}
    });
    let added = call(&tool, add_request.clone()).await;
    let replayed = call(&tool, add_request).await;

    assert_eq!(added["status"], "connected");
    assert_eq!(added["name"], "Exa MCP Server");
    assert_eq!(added["tools"], 1);
    assert_eq!(replayed["package_digest"], added["package_digest"]);
    assert_eq!(replayed["connection"], added["connection"]);
    assert_eq!(
        fs::read_to_string(actions).expect("read catalog actions"),
        "search\nresolve\nresolve\n"
    );
    assert_eq!(
        manager
            .list()
            .await
            .expect("list idempotent catalog package")
            .len(),
        1
    );
    let tools = mcp
        .alpha_tool_summaries(crate::ALPHA_PROFILE_ID)
        .expect("read hot-loaded tools");
    assert_eq!(tools.len(), 1);
}

#[tokio::test]
async fn failed_catalog_connection_keeps_the_package_but_does_not_enable_tools() {
    let directory = tempdir().expect("temporary failed catalog addition fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let catalog_adapter = directory.path().join("catalog.mjs");
    write_single_candidate_catalog_adapter(&catalog_adapter, CATALOG_REFERENCE);
    let mcp_adapter = directory.path().join("failed-mcp.mjs");
    write_failed_mcp_adapter(&mcp_adapter);
    let skills = test_skill_store(&database, directory.path());
    let manager = PluginManager::initialize(
        database,
        directory.path().join("installed"),
        mcp.clone(),
        Some(mcp_adapter),
        McpCredentialResolver::default(),
        Some(IntegrationCatalog::new(catalog_adapter)),
        skills,
    )
    .expect("initialize extension manager");
    let tool = ManageTool::new(manager.clone(), directory.path().to_path_buf());

    let result = invoke_tool(
        Some(&tool),
        ToolCall {
            id: "failed-catalog-add".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: json!({
                "action": "add",
                "source": {"kind": "catalog", "candidate": CATALOG_REFERENCE}
            }),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("remote discovery failure is definite");

    assert!(result.is_error);
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("failed addition must return one model-visible error")
    };
    let error: Value = serde_json::from_str(text)
        .unwrap_or_else(|source| panic!("decode addition error from {text:?}: {source}"));
    assert_eq!(error["code"], "mcp_incompatible_protocol");
    assert_eq!(
        error["message"],
        "The endpoint supports no usable MCP version."
    );
    assert_eq!(error["status"], "installed_connection_failed");
    let package_digest = error["package_digest"]
        .as_str()
        .expect("failed connection reports the installed package");
    let installed = manager.list().await.expect("list installed packages");
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].digest(), package_digest);
    assert!(
        mcp.alpha_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read Alpha registry")
            .is_empty()
    );
}

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
        McpCredentialResolver::default(),
        None,
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

    assert_eq!(added["status"], "connected");
    assert_eq!(added["source"], "mcp");
    assert_eq!(added["tools"], 1);
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
        McpCredentialResolver::default(),
        None,
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
  wire_version: 4,
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
        McpCredentialResolver::default(),
        None,
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
    assert!(matches!(
        mcp.connection_config("exa"),
        Err(crate::mcp::McpHostError::NotFound(_))
    ));
    assert!(
        mcp.alpha_tool_summaries(crate::ALPHA_PROFILE_ID)
            .expect("read Alpha registry")
            .is_empty()
    );
}

#[tokio::test]
async fn catalog_failures_keep_the_safe_exact_message_and_recovery_hint() {
    let directory = tempdir().expect("temporary catalog error fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let adapter = directory.path().join("failed-catalog.mjs");
    fs::write(
        &adapter,
        r"
for await (const _chunk of process.stdin) {}
process.stdout.write(JSON.stringify({
  wire_version: 1,
  event: 'failed',
  failure: {
    kind: 'unavailable',
    message: 'integrations.sh returned HTTP 503.',
    diagnostic: {code: 'catalog_http_error', http_status: 503, detail: 'service unavailable'}
  }
}) + '\n');
",
    )
    .expect("write failed catalog adapter");
    let skills = test_skill_store(&database, directory.path());
    let manager = PluginManager::initialize(
        database,
        directory.path().join("installed"),
        mcp,
        None,
        McpCredentialResolver::default(),
        Some(IntegrationCatalog::new(adapter)),
        skills,
    )
    .expect("initialize extension manager");
    let tool = ManageTool::new(manager, directory.path().to_path_buf());

    let result = invoke_tool(
        Some(&tool),
        ToolCall {
            id: "catalog-error".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: json!({"action": "search", "query": "web search"}),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("catalog failure is definite");

    assert!(result.is_error);
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("catalog error must return one text block")
    };
    let model: Value = serde_json::from_str(text).expect("decode model-visible catalog error");
    assert_eq!(model["code"], "integration_catalog_unavailable");
    assert_eq!(model["message"], "integrations.sh returned HTTP 503.");
    assert_eq!(model["retryable"], true);
    let details = result.details.expect("catalog error keeps Host details");
    assert_eq!(
        details["integration_catalog"]["failure"]["diagnostic"]["http_status"],
        503
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
