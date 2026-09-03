use std::fs;

use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::super::{ManageTool, TOOL_NAME};
use crate::{
    ALPHA_PROFILE_ID, AgentProfileId,
    host::catalog,
    mcp::{McpCatalogStore, McpCredentialResolver, McpHostError},
    plugins::{PluginCredential, PluginManager, tests::test_skill_store},
};

const ORIGINAL_ENDPOINT: &str = "https://original.example/mcp";
const REPLACEMENT_ENDPOINT: &str = "https://replacement.example/mcp";

#[tokio::test]
async fn failed_discovery_leaves_no_active_connection_configuration() {
    let directory = tempdir().expect("temporary extension error fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let adapter = directory.path().join("failed-adapter.mjs");
    write_failed_adapter(&adapter);
    let manager = manager(&directory, &database, &mcp, adapter);
    let source = directory.path().join("source");
    fs::create_dir(&source).expect("create plugin source");
    crate::plugins::tests::write_exa_plugin(&source, ORIGINAL_ENDPOINT);
    let inspection = manager.inspect(&source).await.expect("inspect package");
    manager
        .install(&source, inspection.digest())
        .await
        .expect("install package");
    let tool = ManageTool::new(manager, directory.path().to_path_buf());

    let result = connect(&tool, inspection.digest(), false).await;

    assert_actionable_failure(&result);
    assert!(matches!(
        mcp.connection_config("exa"),
        Err(McpHostError::NotFound(_))
    ));
    assert!(
        mcp.profile_tool_summaries(ALPHA_PROFILE_ID)
            .expect("read Alpha registry")
            .is_empty()
    );
}

#[tokio::test]
async fn failed_replacement_preserves_the_previous_connection_atomically() {
    let directory = tempdir().expect("temporary replacement fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let working_adapter = directory.path().join("working-adapter.mjs");
    super::support::write_mcp_adapter(&working_adapter);
    let original_manager = manager(&directory, &database, &mcp, working_adapter);
    let original_source = directory.path().join("original");
    fs::create_dir(&original_source).expect("create original plugin source");
    crate::plugins::tests::write_exa_plugin(&original_source, ORIGINAL_ENDPOINT);
    let original_inspection = original_manager
        .inspect(&original_source)
        .await
        .expect("inspect original package");
    original_manager
        .install(&original_source, original_inspection.digest())
        .await
        .expect("install original package");
    let original = original_manager
        .connect_profile(
            &AgentProfileId::new(ALPHA_PROFILE_ID).expect("valid Alpha profile"),
            original_inspection.digest(),
            "exa",
            "exa",
            PluginCredential::None,
            CancellationToken::new(),
        )
        .await
        .expect("connect original package");

    let failed_adapter = directory.path().join("failed-adapter.mjs");
    write_failed_adapter(&failed_adapter);
    let replacement_manager = manager(&directory, &database, &mcp, failed_adapter);
    let replacement_source = directory.path().join("replacement");
    fs::create_dir(&replacement_source).expect("create replacement plugin source");
    crate::plugins::tests::write_exa_plugin(&replacement_source, REPLACEMENT_ENDPOINT);
    let replacement_inspection = replacement_manager
        .inspect(&replacement_source)
        .await
        .expect("inspect replacement package");
    replacement_manager
        .install(&replacement_source, replacement_inspection.digest())
        .await
        .expect("install replacement package");
    let tool = ManageTool::new(replacement_manager, directory.path().to_path_buf());

    let result = connect(&tool, replacement_inspection.digest(), true).await;

    assert_actionable_failure(&result);
    assert_eq!(
        mcp.connection_config("exa")
            .expect("original connection survives")
            .endpoint,
        ORIGINAL_ENDPOINT
    );
    assert_eq!(
        mcp.load_catalog("exa")
            .expect("original catalog survives")
            .digest(),
        original.digest()
    );
    assert_eq!(
        mcp.profile_tool_summaries(ALPHA_PROFILE_ID)
            .expect("original profile attachment survives")
            .len(),
        1
    );
}

fn manager(
    directory: &tempfile::TempDir,
    database: &std::path::Path,
    mcp: &McpCatalogStore,
    adapter: std::path::PathBuf,
) -> PluginManager {
    PluginManager::initialize(
        database.to_path_buf(),
        directory.path().join("installed"),
        mcp.clone(),
        Some(adapter),
        None,
        McpCredentialResolver::default(),
        test_skill_store(database, directory.path()),
    )
    .expect("initialize extension manager")
}

async fn connect(
    tool: &ManageTool,
    package_digest: &str,
    replace: bool,
) -> renoa_agent::ToolResult {
    invoke_tool(
        Some(tool),
        ToolCall {
            id: "connect-error".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: json!({
                "action": "connect",
                "package_digest": package_digest,
                "server": "exa",
                "connection": "exa",
                "replace": replace
            }),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("discovery failure is definite")
}

fn assert_actionable_failure(result: &renoa_agent::ToolResult) {
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
    let details = result
        .details
        .as_ref()
        .expect("Host keeps the safe diagnostic");
    assert_eq!(
        details["mcp"]["failure"]["diagnostic"]["code"],
        "ERA_NEGOTIATION_FAILED"
    );
    assert_eq!(
        details["mcp"]["failure"]["diagnostic"]["detail"],
        "server offered 2023-01-01"
    );
}

fn write_failed_adapter(path: &std::path::Path) {
    fs::write(
        path,
        r#"
for await (const _chunk of process.stdin) {}
process.stdout.write(JSON.stringify({
  wire_version: 8,
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
}
