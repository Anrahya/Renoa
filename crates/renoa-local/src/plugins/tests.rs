use std::{
    fs,
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{PluginCredential, PluginError, manager::PluginManager};
use crate::{
    AgentId,
    host::catalog,
    mcp::{McpCatalogStore, McpCredentialResolver},
    skills::SkillStore,
};

pub(super) fn test_agent_id(seed: u128) -> AgentId {
    crate::derived_agent_id(uuid::Uuid::from_u128(seed))
}

pub(super) fn test_skill_store(database: &Path, root: &Path) -> SkillStore {
    SkillStore::initialize(database.to_path_buf(), root.join("skills"), None)
        .expect("initialize test skill store")
}

#[test]
fn exa_agent_plugin_is_normalized_without_a_provider_specific_path() {
    let directory = tempdir().expect("temporary plugin fixture");
    write_exa_plugin(
        directory.path(),
        "https://mcp.exa.ai/mcp?client=agent-plugin",
    );
    let captured = super::inspect::inspect(directory.path()).expect("inspect Exa-shaped package");
    let inspection = captured.inspection;
    assert_eq!(inspection.metadata.name, "exa");
    assert_eq!(inspection.metadata.version.as_deref(), Some("3.4.1"));
    assert_eq!(inspection.metadata.license.as_deref(), Some("MIT"));
    assert_eq!(inspection.mcp_servers.len(), 1);
    assert_eq!(inspection.mcp_servers[0].id, "exa");
    assert_eq!(
        inspection.mcp_servers[0]
            .request_headers
            .get("x-exa-source")
            .map(String::as_str),
        Some("agent-plugin")
    );
    assert!(inspection.notices.is_empty());
}

#[test]
fn manifest_failure_is_fatal_but_one_bad_mcp_entry_is_isolated() {
    let directory = tempdir().expect("temporary plugin fixture");
    write_exa_plugin(directory.path(), "https://mcp.exa.ai/mcp");
    fs::write(
        directory.path().join("mcp.json"),
        serde_json::to_vec(&json!({
            "$schema": super::inspect::MCP_SCHEMA,
            "mcpServers": {
                "bad": {
                    "type": "streamable-http",
                    "url": "https://bad.example/mcp",
                    "headers": {"Authorization": "package-secret"}
                },
                "good": {
                    "type": "streamable-http",
                    "url": "https://good.example/mcp"
                }
            }
        }))
        .expect("encode MCP fixture"),
    )
    .expect("write MCP fixture");
    let inspection = super::inspect::inspect(directory.path())
        .expect("one bad MCP entry must not reject the plugin")
        .inspection;
    assert_eq!(
        inspection
            .mcp_servers
            .iter()
            .map(|server| server.id.as_str())
            .collect::<Vec<_>>(),
        ["good"]
    );
    assert_eq!(inspection.notices.len(), 1);
    assert_eq!(inspection.notices[0].entry(), Some("bad"));

    fs::write(
        directory.path().join("plugin.json"),
        br#"{"$schema":"wrong","name":"exa"}"#,
    )
    .expect("write invalid manifest");
    assert!(matches!(
        super::inspect::inspect(directory.path()),
        Err(PluginError::Invalid(_))
    ));
}

#[test]
fn invalid_plugin_names_explain_the_agent_plugins_rule() {
    let directory = tempdir().expect("temporary plugin fixture");
    fs::write(
        directory.path().join("plugin.json"),
        serde_json::to_vec(&json!({
            "$schema": super::inspect::PLUGIN_SCHEMA,
            "name": "Notion MCP"
        }))
        .expect("encode invalid manifest"),
    )
    .expect("write invalid manifest");

    let error = super::inspect::inspect(directory.path())
        .expect_err("invalid plugin name must be rejected")
        .to_string();
    assert!(error.contains("start and end with a lowercase ASCII letter or digit"));
    assert!(error.contains("cannot contain '..' or '--'"));
}

#[cfg(unix)]
#[test]
fn symlinked_fixed_components_are_denied_at_the_narrowest_boundary() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().expect("temporary plugin fixture");
    let external = directory.path().join("external.json");
    fs::write(&external, b"{}").expect("write external fixture");
    let plugin = directory.path().join("plugin");
    fs::create_dir(&plugin).expect("create plugin root");
    write_manifest(&plugin);
    symlink(&external, plugin.join("mcp.json")).expect("symlink MCP component");
    let inspection = super::inspect::inspect(&plugin)
        .expect("symlinked optional MCP must be isolated")
        .inspection;
    assert!(inspection.mcp_servers.is_empty());
    assert!(inspection.notices.iter().any(|notice| {
        notice.component() == "mcp" && notice.reason().contains("not a real file")
    }));

    fs::remove_file(plugin.join("plugin.json")).expect("remove real manifest");
    symlink(&external, plugin.join("plugin.json")).expect("symlink manifest");
    assert!(matches!(
        super::inspect::inspect(&plugin),
        Err(PluginError::Invalid(_))
    ));
}

#[tokio::test]
async fn api_key_plugin_connects_and_hot_loads_without_persisting_the_secret() {
    let repository = workspace_root();
    let adapter = compiled_adapter(&repository);
    let directory = tempdir().expect("temporary extension fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    crate::test_agents::insert_agent(&database, &test_agent_id(1).to_string());
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let agent_id = test_agent_id(1);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind MCP fixture");
    let address = listener.local_addr().expect("MCP fixture address");
    let endpoint = format!("http://127.0.0.1:{}/mcp", address.port());
    let server = thread::spawn(move || serve_authenticated_discovery(&listener));

    let plugin = directory.path().join("exa-plugin");
    fs::create_dir(&plugin).expect("create Exa plugin fixture");
    write_exa_plugin(&plugin, &endpoint);
    let secret_fixture = tempdir().expect("temporary Secret Service fixture");
    let arguments = secret_fixture.path().join("arguments.txt");
    let secret_tool = compile_secret_tool(secret_fixture.path(), &arguments);
    let resolver = McpCredentialResolver::with_executables(
        secret_fixture.path().join("unused-gh"),
        secret_tool,
    );
    let skills = test_skill_store(&database, directory.path());
    let manager = PluginManager::initialize(
        database.clone(),
        directory.path().join("plugins"),
        mcp.clone(),
        Some(adapter),
        None,
        resolver,
        skills,
    )
    .expect("initialize plugin manager");
    assert!(
        mcp.agent_tool_summaries(&agent_id.to_string())
            .expect("read empty registry")
            .is_empty()
    );
    let inspection = manager.inspect(&plugin).await.expect("inspect package");
    manager
        .install(&plugin, inspection.digest())
        .await
        .expect("install package");

    let snapshot = manager
        .connect_profile(
            &agent_id,
            inspection.digest(),
            "exa",
            "exa.default",
            PluginCredential::SecretServiceHeader {
                credential_id: "exa.default".to_owned(),
                header: "X-API-Key".to_owned(),
                prefix: String::new(),
            },
            CancellationToken::new(),
        )
        .await
        .expect("connect Exa-shaped package");
    let requests = server.join().expect("join MCP fixture");

    assert_eq!(snapshot.tools().len(), 1);
    assert_eq!(requests, ["server/discover", "tools/list"]);
    assert_eq!(
        fs::read_to_string(arguments).expect("read Secret Service arguments"),
        "lookup\napplication\nrenoa\ncredential\nexa.default"
    );
    let hot_loaded = mcp
        .agent_tool_summaries(&agent_id.to_string())
        .expect("same registry object sees new connection");
    assert_eq!(hot_loaded.len(), 1);
    assert_eq!(snapshot.tools()[0].name(), "web_search_exa");
    assert_no_secret_in_database(&database, "exa-fixture-api-key");
}

#[tokio::test]
async fn stdio_plugin_package_discovers_and_executes_end_to_end() {
    let repository = workspace_root();
    let adapter = compiled_adapter(&repository);
    let directory = tempdir().expect("temporary stdio extension fixture");
    let (agent_id, mcp, manager) = init_stdio_manager(directory.path(), adapter.clone());
    let package_dir = directory.path().join("browse-fixture");
    write_stdio_browse_fixture(&package_dir);

    let inspected = manager
        .inspect(&package_dir)
        .await
        .expect("inspect stdio package");
    assert!(inspected.notices().is_empty());
    assert_eq!(inspected.mcp_servers().len(), 1);
    assert!(inspected.mcp_servers()[0].is_stdio());

    let bad_cred = manager
        .invoke(
            &agent_id,
            directory.path(),
            super::api::PluginRequest::Add {
                source: super::api::PluginSource::Package {
                    source_path: package_dir.clone(),
                },
                expected_digest: Some(inspected.digest().to_owned()),
                server: Some("browse".to_owned()),
                connection: Some("browse.bad".to_owned()),
                credential: Some(super::api::PluginAuthentication::OAuth {}),
                replace: false,
            },
            super::api::PluginInvocation {
                operation_id: "stdio-bad-cred",
                updates: None,
                cancellation: CancellationToken::new(),
            },
        )
        .await;
    assert!(bad_cred.is_err(), "OAuth on stdio must be rejected");
    assert!(
        mcp.agent_tool_summaries(&agent_id.to_string())
            .unwrap()
            .is_empty()
    );

    let added = manager
        .invoke(
            &agent_id,
            directory.path(),
            super::api::PluginRequest::Add {
                source: super::api::PluginSource::Package {
                    source_path: package_dir,
                },
                expected_digest: Some(inspected.digest().to_owned()),
                server: Some("browse".to_owned()),
                connection: Some("browse.local".to_owned()),
                credential: None,
                replace: false,
            },
            super::api::PluginInvocation {
                operation_id: "stdio-pkg-add",
                updates: None,
                cancellation: CancellationToken::new(),
            },
        )
        .await
        .expect("add stdio package");
    let super::api::PluginOutcome::Added(outcome) = added else {
        panic!("expected Added outcome");
    };
    assert!(matches!(
        outcome.connection,
        super::PluginConnectionOutcome::Connected { .. }
    ));

    let parsed = execute_stdio_ping(&mcp, agent_id, directory.path(), adapter, "hello-stdio").await;
    assert_eq!(parsed["label"], "hello-stdio");
    let expected_root = directory.path().join("plugins").join(inspected.digest());
    assert_eq!(parsed["plugin_root"].as_str(), expected_root.to_str());
    assert_eq!(parsed["cwd"], parsed["plugin_root"]);

    // Verify stored integration endpoint matches the RESOLVED stdio config digest,
    // distinct from the unresolved package mcp.json endpoint.
    let stored = mcp
        .connection_config("browse.local")
        .expect("stored stdio connection config");
    assert_ne!(
        stored.endpoint,
        inspected.mcp_servers()[0].endpoint(),
        "resolved integration endpoint must bind the resolved paths, not unresolved placeholders"
    );
    assert_eq!(
        stored.endpoint,
        stored
            .stdio
            .as_ref()
            .expect("stored stdio")
            .canonical_endpoint()
            .expect("canonical endpoint")
    );
}

#[tokio::test]
async fn stdio_plugin_data_dir_is_created_only_on_valid_connect_and_never_on_rejected_preflight() {
    let repository = workspace_root();
    let adapter = compiled_adapter(&repository);
    let directory = tempdir().expect("temporary plugin_data stdio fixture");
    let (agent_id, mcp, manager) = init_stdio_manager(directory.path(), adapter.clone());
    let package_dir = directory.path().join("plugin-data-fixture");
    write_stdio_browse_fixture(&package_dir);
    fs::write(
        package_dir.join("mcp.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": super::inspect::MCP_SCHEMA,
            "mcpServers": {
                "browse": {
                    "type": "stdio",
                    "command": "node",
                    "args": ["${PLUGIN_ROOT}/server.mjs"],
                    "cwd": "${PLUGIN_DATA}"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let inspected = manager
        .inspect(&package_dir)
        .await
        .expect("inspect PLUGIN_DATA stdio package");
    let data_dir = directory
        .path()
        .join("plugin-data")
        .join(inspected.digest());

    // Add package without connecting yet.
    manager
        .invoke(
            &agent_id,
            directory.path(),
            super::api::PluginRequest::Add {
                source: super::api::PluginSource::Package {
                    source_path: package_dir,
                },
                expected_digest: Some(inspected.digest().to_owned()),
                server: None,
                connection: None,
                credential: None,
                replace: false,
            },
            super::api::PluginInvocation {
                operation_id: "stdio-data-add-only",
                updates: None,
                cancellation: CancellationToken::new(),
            },
        )
        .await
        .expect("install package without connecting");
    assert!(
        !data_dir.exists(),
        "install alone must not create plugin-data"
    );

    for (op, restart, scope) in [
        ("stdio-data-bad-restart", true, None),
        ("stdio-data-bad-scope", false, Some("read")),
    ] {
        let rejected = manager
            .connect_profile_operation(
                super::manager::ProfileConnectionRequest {
                    agent_id: &agent_id,
                    package_digest: inspected.digest(),
                    server_id: "browse",
                    connection_id: "browse.data",
                    credential: super::PluginCredential::None,
                    replace: false,
                    restart,
                    requested_scope: scope,
                    operation_id: op,
                    updates: None,
                },
                CancellationToken::new(),
            )
            .await;
        assert!(
            matches!(rejected, Err(super::PluginError::Invalid(_))),
            "{op} must be rejected: {rejected:?}"
        );
        assert!(!data_dir.exists(), "{op} must not create plugin-data");
    }

    // Valid connect creates plugin-data/<digest> and executes with cwd == plugin-data/<digest>.
    manager
        .connect_profile(
            &agent_id,
            inspected.digest(),
            "browse",
            "browse.data",
            super::PluginCredential::None,
            CancellationToken::new(),
        )
        .await
        .expect("connect PLUGIN_DATA stdio package");
    assert!(
        data_dir.is_dir(),
        "valid connect must create plugin-data dir"
    );

    let parsed = execute_stdio_ping(&mcp, agent_id, directory.path(), adapter, "data-cwd").await;
    assert_eq!(parsed["cwd"].as_str(), data_dir.to_str());
}

#[tokio::test]
async fn direct_stdio_mcp_source_adds_and_connects_end_to_end() {
    let repository = workspace_root();
    let adapter = compiled_adapter(&repository);
    let directory = tempdir().expect("temporary direct stdio fixture");
    let (agent_id, _mcp, manager) = init_stdio_manager(directory.path(), adapter);
    let package_dir = directory.path().join("browse-fixture");
    write_stdio_browse_fixture(&package_dir);

    let direct = manager
        .invoke(
            &agent_id,
            directory.path(),
            super::api::PluginRequest::Add {
                source: super::api::PluginSource::StdioMcp {
                    name: "direct-stdio".to_owned(),
                    description: "Direct stdio MCP fixture.".to_owned(),
                    server: "direct".to_owned(),
                    command: "node".to_owned(),
                    args: vec![
                        package_dir
                            .join("server.mjs")
                            .to_string_lossy()
                            .into_owned(),
                    ],
                    env: std::collections::BTreeMap::new(),
                    cwd: None,
                    documentation: Some("https://opencode.ai/browse".to_owned()),
                },
                expected_digest: None,
                server: None,
                connection: Some("direct.local".to_owned()),
                credential: None,
                replace: false,
            },
            super::api::PluginInvocation {
                operation_id: "stdio-direct-add",
                updates: None,
                cancellation: CancellationToken::new(),
            },
        )
        .await
        .expect("add direct stdio_mcp source");
    let super::api::PluginOutcome::Added(direct_outcome) = direct else {
        panic!("expected Added outcome");
    };
    assert!(matches!(
        direct_outcome.source,
        super::PluginSourceReceipt::StdioMcp
    ));
    assert!(matches!(
        direct_outcome.connection,
        super::PluginConnectionOutcome::Connected { .. }
    ));
}

fn init_stdio_manager(
    root: &Path,
    adapter: PathBuf,
) -> (renoa_kernel::AgentId, McpCatalogStore, PluginManager) {
    let database = root.join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let agent_id = test_agent_id(7);
    crate::test_agents::insert_agent(&database, &agent_id.to_string());
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let skills = test_skill_store(&database, root);
    let manager = PluginManager::initialize(
        database,
        root.join("plugins"),
        mcp.clone(),
        Some(adapter),
        None,
        McpCredentialResolver::with_executables(
            root.join("unused-gh"),
            root.join("unused-secret-tool"),
        ),
        skills,
    )
    .expect("initialize plugin manager");
    (agent_id, mcp, manager)
}

fn write_stdio_browse_fixture(package_dir: &Path) {
    fs::create_dir_all(package_dir.join("skills/browse")).expect("create skill dir");
    fs::write(
        package_dir.join("server.mjs"),
        r#"
import { createInterface } from "node:readline";
const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
const send = (msg) => process.stdout.write(JSON.stringify(msg) + "\n");
rl.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  const msg = JSON.parse(trimmed);
  if (msg.id === undefined) return;
  if (msg.method === "server/discover") {
    send({
      jsonrpc: "2.0",
      id: msg.id,
      result: {
        resultType: "complete",
        supportedVersions: ["2026-07-28"],
        capabilities: { tools: {} },
        ttlMs: 0,
        cacheScope: "private",
        _meta: { "io.modelcontextprotocol/serverInfo": { name: "fixture-browse", version: "0.1.0" } }
      }
    });
  } else if (msg.method === "tools/list") {
    send({
      jsonrpc: "2.0",
      id: msg.id,
      result: {
        resultType: "complete",
        ttlMs: 0,
        cacheScope: "private",
        tools: [{
          name: "ping_browser",
          description: "Return cwd and plugin root.",
          inputSchema: {
            type: "object",
            properties: { label: { type: "string" } },
            required: ["label"]
          }
        }]
      }
    });
  } else if (msg.method === "tools/call") {
    send({
      jsonrpc: "2.0",
      id: msg.id,
      result: {
        resultType: "complete",
        isError: false,
        content: [{
          type: "text",
          text: JSON.stringify({
            label: msg.params.arguments.label,
            cwd: process.cwd(),
            plugin_root: process.env.PLUGIN_ROOT ?? null
          })
        }]
      }
    });
  }
});
"#,
    )
    .expect("write stdio server script");
    fs::write(
        package_dir.join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": super::inspect::PLUGIN_SCHEMA,
            "name": "fixture.browse",
            "version": "0.1.0",
            "description": "Fixture stdio browser plugin."
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        package_dir.join("mcp.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": super::inspect::MCP_SCHEMA,
            "mcpServers": {
                "browse": {
                    "type": "stdio",
                    "command": "node",
                    "args": ["${PLUGIN_ROOT}/server.mjs"],
                    "cwd": "${PLUGIN_ROOT}"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        package_dir.join("skills/browse/SKILL.md"),
        "---\nname: browse\ndescription: Fixture browse skill.\n---\nUse ping_browser.\n",
    )
    .unwrap();
}

async fn execute_stdio_ping(
    mcp: &McpCatalogStore,
    agent_id: renoa_kernel::AgentId,
    root: &Path,
    adapter: PathBuf,
    label: &str,
) -> serde_json::Value {
    let summaries = mcp
        .agent_tool_summaries(&agent_id.to_string())
        .expect("read agent MCP summaries");
    assert_eq!(summaries.len(), 1);
    let reference = summaries[0].reference().expect("tool reference");
    assert_eq!(reference.tool_name(), "ping_browser");
    let bindings = crate::mcp::agent_registry_bindings(
        agent_id,
        mcp.clone(),
        Some(adapter.clone()),
        crate::mcp::McpAuthorizationResolver::new(
            mcp,
            Some(adapter),
            McpCredentialResolver::with_executables(
                root.join("unused-gh"),
                root.join("unused-secret-tool"),
            ),
        ),
        renoa_kernel::SessionId::new(),
        None,
    );
    let execute_tool = bindings
        .into_iter()
        .find(|binding| binding.tool().spec().name == "tool_execute")
        .expect("tool_execute binding");
    let output = renoa_agent::invoke_tool(
        Some(execute_tool.tool().as_ref()),
        renoa_agent::ToolCall {
            id: "call-stdio".to_owned(),
            name: "tool_execute".to_owned(),
            arguments: json!({
                "reference": reference.to_string(),
                "arguments": { "label": label }
            }),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("execute stdio tool");
    assert!(!output.is_error, "stdio tool call failed: {output:?}");
    let text = output
        .content
        .iter()
        .find_map(|block| match block {
            renoa_agent::ContentBlock::Text { text } => Some(text.as_str()),
            renoa_agent::ContentBlock::Image { .. } => None,
        })
        .expect("text result");
    serde_json::from_str(text).expect("json tool output")
}

pub(super) fn write_exa_plugin(root: &Path, endpoint: &str) {
    write_manifest(root);
    fs::write(
        root.join("mcp.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": super::inspect::MCP_SCHEMA,
            "mcpServers": {
                "exa": {
                    "type": "streamable-http",
                    "url": endpoint,
                    "headers": {"x-exa-source": "agent-plugin"}
                }
            }
        }))
        .expect("encode MCP fixture"),
    )
    .expect("write MCP fixture");
}

fn write_manifest(root: &Path) {
    fs::write(
        root.join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": super::inspect::PLUGIN_SCHEMA,
            "name": "exa",
            "version": "3.4.1",
            "description": "Web search and research through Exa.",
            "repository": "https://github.com/exa-labs/exa-mcp-server",
            "license": "MIT"
        }))
        .expect("encode plugin fixture"),
    )
    .expect("write plugin fixture");
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("renoa-local is two levels below workspace root")
        .to_path_buf()
}

fn compiled_adapter(workspace: &Path) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let root = workspace.join("adapters/mcp-client-node");
            let adapter = root.join("dist/src/main.js");
            let status = Command::new("pnpm")
                .args([
                    "--dir",
                    root.to_str().expect("adapter path is UTF-8"),
                    "build",
                ])
                .status()
                .expect("build MCP adapter");
            assert!(status.success(), "MCP adapter build failed: {status}");
            adapter
        })
        .clone()
}

fn compile_secret_tool(directory: &Path, arguments: &Path) -> PathBuf {
    let source = directory.join("secret-tool.rs");
    let executable = directory.join(if cfg!(windows) {
        "secret-tool.exe"
    } else {
        "secret-tool"
    });
    fs::write(
        &source,
        format!(
            r#"
fn main() {{
    let arguments = std::env::args().skip(1).collect::<Vec<_>>().join("\n");
    std::fs::write({arguments:?}, arguments).expect("write arguments");
    println!("exa-fixture-api-key");
}}
"#
        ),
    )
    .expect("write fake secret-tool source");
    let status = Command::new("rustc")
        .args(["--edition", "2024"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("compile fake secret-tool");
    assert!(status.success(), "secret-tool compilation failed: {status}");
    executable
}

fn serve_authenticated_discovery(listener: &TcpListener) -> Vec<String> {
    let mut methods = Vec::new();
    for expected in ["server/discover", "tools/list"] {
        let (mut stream, _) = listener.accept().expect("accept MCP request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set MCP request timeout");
        let request = read_http_request(&mut stream);
        let header_end = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("MCP HTTP body");
        let headers = String::from_utf8_lossy(&request[..header_end]);
        assert_header(&headers, "x-api-key", "exa-fixture-api-key");
        assert_header(&headers, "x-exa-source", "agent-plugin");
        let rpc: Value = serde_json::from_slice(&request[header_end + 4..])
            .expect("decode MCP JSON-RPC request");
        assert_eq!(rpc["method"], expected);
        methods.push(expected.to_owned());
        let result = if expected == "server/discover" {
            json!({
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}}
            })
        } else {
            json!({
                "resultType": "complete",
                "tools": [{
                    "name": "web_search_exa",
                    "description": "Search the web.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"query": {"type": "string"}},
                        "required": ["query"]
                    }
                }],
                "ttlMs": 0,
                "cacheScope": "private"
            })
        };
        let response = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": rpc["id"],
            "result": result
        }))
        .expect("encode MCP response");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        )
        .expect("write MCP response headers");
        stream.write_all(&response).expect("write MCP response");
        stream.flush().expect("flush MCP response");
    }
    methods
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4 * 1_024];
    let mut expected = None;
    loop {
        let read = stream.read(&mut buffer).expect("read MCP request");
        assert_ne!(read, 0, "MCP request closed before its body arrived");
        request.extend_from_slice(&buffer[..read]);
        if expected.is_none()
            && let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
        {
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .expect("MCP Content-Length");
            expected = Some(header_end + 4 + content_length);
        }
        if expected.is_some_and(|expected| request.len() >= expected) {
            return request;
        }
    }
}

fn assert_header(headers: &str, expected_name: &str, expected_value: &str) {
    assert!(headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case(expected_name) && value.trim() == expected_value
        })
    }));
}

fn assert_no_secret_in_database(database: &Path, secret: &str) {
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", database.display()));
        if path.is_file() {
            let bytes = fs::read(&path).expect("read Host database artifact");
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()),
                "credential leaked into {}",
                path.display()
            );
        }
    }
}
