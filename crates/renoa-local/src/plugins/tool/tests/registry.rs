use std::{fs, path::Path};

use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{super::BINDING_REVISION, call};
use crate::{
    host::catalog,
    mcp::{McpCatalogStore, McpCredentialResolver},
    plugins::{PluginManager, tests::test_skill_store},
};

#[test]
fn shared_package_synchronization_changes_the_frozen_extension_contract() {
    assert_eq!(BINDING_REVISION, "renoa-extension-manager-v13");
}

#[tokio::test]
async fn search_and_exact_lookup_cross_the_real_adapter_boundary_without_installing() {
    let directory = tempdir().expect("temporary Registry fixture");
    let adapter = directory.path().join("registry.mjs");
    write_registry_adapter(&adapter);
    let manager = manager(directory.path(), Some(adapter));
    let tool = super::super::ManageTool::new(manager.clone(), directory.path().to_path_buf());

    let search = call(
        &tool,
        json!({"action": "search", "query": "install Cloudflare MCP"}),
    )
    .await;
    assert_eq!(search["action"], "search");
    assert_eq!(search["installed"], false);
    assert_eq!(search["source"], "official_mcp_registry");
    assert_eq!(search["query"], "install Cloudflare MCP");
    assert_eq!(
        search["candidates"][0]["registry_name"],
        "com.cloudflare.mcp/mcp"
    );
    assert_eq!(
        search["candidates"][0]["publisher_description"],
        "Publisher supplied Cloudflare description."
    );
    assert!(search["candidates"][0].get("remotes").is_none());
    assert_eq!(search["trust"]["verified"], "publisher_namespace_control");

    let lookup = call(
        &tool,
        json!({
            "action": "lookup",
            "registry_name": "com.cloudflare.mcp/mcp",
            "registry_version": "1.0.0"
        }),
    )
    .await;
    assert_eq!(lookup["action"], "lookup");
    assert_eq!(lookup["installed"], false);
    assert_eq!(lookup["record"]["registry_name"], "com.cloudflare.mcp/mcp");
    assert_eq!(lookup["record"]["status"], "deleted");
    assert_eq!(lookup["record"]["remotes"][0]["transport_supported"], true);
    assert_eq!(lookup["record"]["remotes"][0]["headers"][0]["secret"], true);
    assert!(
        lookup["next_action"]
            .as_str()
            .is_some_and(|value| value.contains("official HTTPS documentation"))
    );
    assert!(
        manager
            .list()
            .await
            .expect("list packages after discovery")
            .is_empty(),
        "read-only Registry discovery must never install a package"
    );
}

#[tokio::test]
async fn registry_http_status_and_safe_message_reach_the_model() {
    let directory = tempdir().expect("temporary Registry error fixture");
    let adapter = directory.path().join("registry-error.mjs");
    write_registry_error_adapter(&adapter);
    let manager = manager(directory.path(), Some(adapter));
    let tool = super::super::ManageTool::new(manager, directory.path().to_path_buf());

    let output = invoke_tool(
        Some(&tool),
        ToolCall {
            id: "registry-failure".to_owned(),
            name: super::super::TOOL_NAME.to_owned(),
            arguments: json!({"action": "search", "query": "cloudflare"}),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("Registry failure must remain a definite model-visible tool result");

    assert!(output.is_error);
    let [ContentBlock::Text { text }] = output.content.as_slice() else {
        panic!("Registry failure must contain one text block")
    };
    let model: Value = serde_json::from_str(text).expect("decode model Registry failure");
    assert_eq!(model["code"], "mcp_registry_unavailable");
    assert_eq!(
        model["message"],
        "Official MCP Registry returned HTTP 429; no extension was installed."
    );
    assert_eq!(model["retryable"], true);
    assert_eq!(model["installed"], false);
    let details = output.details.expect("Host keeps Registry diagnostics");
    assert_eq!(details["registry"]["failure"]["http_status"], 429);
    assert_eq!(
        details["registry"]["failure"]["detail"],
        "Official MCP Registry returned HTTP 429; no extension was installed."
    );
}

#[tokio::test]
async fn missing_registry_adapter_is_a_model_visible_configuration_failure() {
    let directory = tempdir().expect("temporary missing Registry fixture");
    let manager = manager(directory.path(), None);
    let tool = super::super::ManageTool::new(manager, directory.path().to_path_buf());

    let output = invoke_tool(
        Some(&tool),
        ToolCall {
            id: "missing-registry".to_owned(),
            name: super::super::TOOL_NAME.to_owned(),
            arguments: json!({"action": "search", "query": "exa"}),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("missing Registry adapter remains a definite tool result");
    assert!(output.is_error);
    let [ContentBlock::Text { text }] = output.content.as_slice() else {
        panic!("missing Registry adapter must contain one text block")
    };
    let model: Value = serde_json::from_str(text).expect("decode configuration failure");
    assert_eq!(model["code"], "mcp_registry_unavailable");
    assert_eq!(model["retryable"], false);
    assert!(
        model["message"]
            .as_str()
            .is_some_and(|value| value.contains("RENOA_MCP_REGISTRY_ADAPTER"))
    );
}

#[tokio::test]
async fn cancellation_stops_before_the_registry_process_starts() {
    let directory = tempdir().expect("temporary cancelled Registry fixture");
    let adapter = directory.path().join("missing-process.mjs");
    fs::write(&adapter, "process.exitCode = 99;").expect("write cancellation sentinel");
    let manager = manager(directory.path(), Some(adapter));
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = manager
        .search_registry("cloudflare", cancellation)
        .await
        .expect_err("cancelled discovery must not run");
    assert!(matches!(
        error,
        crate::plugins::discovery::RegistryError::Cancelled
    ));
}

fn manager(path: &Path, registry_adapter: Option<std::path::PathBuf>) -> PluginManager {
    let database = path.join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let skills = test_skill_store(&database, path);
    PluginManager::initialize(
        database,
        path.join("installed"),
        mcp,
        None,
        registry_adapter,
        McpCredentialResolver::default(),
        skills,
    )
    .expect("initialize extension manager")
}

fn write_registry_adapter(path: &Path) {
    fs::write(
        path,
        r#"
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
const trust = {
  verified: 'publisher_namespace_control',
  not_verified: ['provider_endorsement', 'metadata_accuracy', 'server_safety', 'endpoint_behavior']
};
let result;
if (request.action === 'search') {
  result = {
    action: 'search', source: 'official_mcp_registry', query: request.query,
    normalized_queries: ['cloudflare'],
    candidates: [{
      registry_name: 'com.cloudflare.mcp/mcp', registry_version: '1.0.0',
      publisher_description: 'Publisher supplied Cloudflare description.',
      publisher: {namespace: 'com.cloudflare.mcp', verification: 'domain'},
      publisher_namespace_matches_query: true, status: 'active', remote_count: 1,
      streamable_http_count: 1, package_count: 0
    }],
    coverage: {returned: 1, unique_seen: 1, rejected_records: 0, filtered_records: 0, source_truncated: false, output_truncated: false},
    trust,
    next_action: 'Call lookup with one exact registry_name and registry_version. Registry publication proves namespace control only; verify provider ownership, endpoint, and authentication in official provider documentation before add.'
  };
} else {
  result = {
    action: 'lookup', source: 'official_mcp_registry',
    record: {
      registry_name: request.registry_name, registry_version: request.registry_version,
      publisher_description: 'Publisher supplied Cloudflare description.',
      publisher: {namespace: 'com.cloudflare.mcp', verification: 'domain'}, status: 'deleted',
      remotes: [{
        transport: 'streamable-http', url: 'https://docs.mcp.cloudflare.com/mcp',
        transport_supported: true,
        headers: [{name: 'Authorization', required: false, secret: true, description: 'Optional token'}]
      }],
      packages: [],
      source_record: `https://registry.modelcontextprotocol.io/v0.1/servers/${encodeURIComponent(request.registry_name)}/versions/${encodeURIComponent(request.registry_version)}`
    },
    trust,
    next_action: "Treat this as publisher metadata only. Verify the selected endpoint and authentication against the provider's official HTTPS documentation. Then call add with kind=mcp and the exact reviewed values; never copy secret header values from registry metadata."
  };
}
process.stdout.write(JSON.stringify({wire_version: 1, event: 'completed', adapter_revision: 'mcp-registry-node-v0.1.0', result}) + '\n');
"#,
    )
    .expect("write Registry adapter");
}

fn write_registry_error_adapter(path: &Path) {
    fs::write(
        path,
        r"
for await (const _chunk of process.stdin) {}
process.stdout.write(JSON.stringify({
  wire_version: 1,
  event: 'failed',
  failure: {
    kind: 'unavailable',
    message: 'Official MCP Registry returned HTTP 429; no extension was installed.',
    diagnostic: {
      code: 'registry_http_error',
      http_status: 429,
      detail: 'Official MCP Registry returned HTTP 429; no extension was installed.'
    }
  }
}) + '\n');
",
    )
    .expect("write Registry error adapter");
}
