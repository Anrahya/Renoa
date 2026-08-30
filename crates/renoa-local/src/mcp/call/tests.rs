use std::{fs, path::Path, time::Duration};

use serde_json::json;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{CALL_BOUNDARY_REVISION, WIRE_VERSION, call_tool, wire::McpCallResult};
use crate::mcp::{
    MCP_ADAPTER_REVISION, MCP_PROTOCOL_VERSION, McpCatalogTool, McpConnectionAuth,
    McpCredentialHeader, McpOutcomeCertainty, ResolvedMcpTool,
};

#[test]
fn frozen_call_revision_tracks_the_process_wire() {
    assert!(
        CALL_BOUNDARY_REVISION.contains(&format!("wire-{WIRE_VERSION}")),
        "call binding revision must change with its process wire"
    );
}

#[tokio::test]
async fn a_valid_terminal_result_survives_nonzero_hung_cleanup() {
    let directory = tempdir().expect("temporary adapter directory");
    let adapter = write_adapter(
        directory.path(),
        r#"
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
if (
  request.action !== "call" ||
  request.protocol_version !== "2026-07-28" ||
  request.tool.name !== "echo" ||
  request.arguments.text !== "hello"
) process.exit(2);
process.stdout.write(JSON.stringify({ wire_version: 7, event: "dispatch_started" }) + "\n");
process.stdout.write(JSON.stringify({
  wire_version: 7,
  event: "completed",
  result: {
    content: [{ type: "text", text: "hello" }],
    structured_content: { present: true, value: { echoed: true } },
    is_error: false
  }
}) + "\n");
process.exitCode = 7;
await new Promise(() => {});
"#,
    );
    let selected = selected_tool("https://example.com/mcp");

    let result = tokio::time::timeout(
        Duration::from_secs(3),
        call_tool(
            &adapter,
            &selected,
            None,
            &json!({"text": "hello"}),
            CancellationToken::new(),
        ),
    )
    .await
    .expect("terminal must stop a hung adapter promptly")
    .expect("valid terminal remains authoritative");

    assert_result(&result);
}

#[tokio::test]
async fn cancellation_after_dispatch_is_an_unknown_outcome_and_reaps_the_adapter() {
    let directory = tempdir().expect("temporary adapter directory");
    let marker = directory.path().join("dispatched");
    let adapter = write_adapter(
        directory.path(),
        &format!(
            r#"
import {{ writeFileSync }} from "node:fs";
for await (const _chunk of process.stdin) {{}}
process.stdout.write(JSON.stringify({{ wire_version: 7, event: "dispatch_started" }}) + "\n");
writeFileSync({}, String(process.pid));
await new Promise(() => {{}});
"#,
            serde_json::to_string(&marker).expect("encode marker path")
        ),
    );
    let selected = selected_tool("https://example.com/mcp");
    let cancellation = CancellationToken::new();
    let running_cancellation = cancellation.clone();
    let running = tokio::spawn(async move {
        call_tool(
            &adapter,
            &selected,
            None,
            &json!({"text": "hello"}),
            running_cancellation,
        )
        .await
    });
    wait_for_path(&marker).await;
    cancellation.cancel();

    let failure = tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .expect("cancellation must settle promptly")
        .expect("join call task")
        .expect_err("lost post-dispatch result is not definite");
    let (_source, certainty, partial_changes_possible) = failure.into_parts();
    assert_eq!(certainty, McpOutcomeCertainty::Unknown);
    assert!(partial_changes_possible);
}

#[tokio::test]
async fn an_exit_before_dispatch_is_a_definite_failure() {
    let directory = tempdir().expect("temporary adapter directory");
    let adapter = write_adapter(
        directory.path(),
        r#"
for await (const _chunk of process.stdin) {}
process.stderr.write("fixture failed before dispatch\n");
process.exit(9);
"#,
    );
    let selected = selected_tool("https://example.com/mcp");

    let failure = call_tool(
        &adapter,
        &selected,
        None,
        &json!({"text": "hello"}),
        CancellationToken::new(),
    )
    .await
    .expect_err("missing terminal must fail");
    let (source, certainty, partial_changes_possible) = failure.into_parts();

    assert_eq!(certainty, McpOutcomeCertainty::Definite);
    assert!(!partial_changes_possible);
    assert!(
        source
            .to_string()
            .contains("fixture failed before dispatch")
    );
}

#[tokio::test]
async fn authorization_is_stdin_only_and_is_redacted_from_completed_output() {
    let directory = tempdir().expect("temporary adapter directory");
    let adapter = write_adapter(
        directory.path(),
        r#"
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
const token = "fixture-secret-token";
if (
  request.credential?.scheme !== "header" ||
  request.credential?.name !== "authorization" ||
  request.credential?.prefix !== "Bearer " ||
  request.credential?.secret !== token ||
  process.argv.slice(2).length !== 0 ||
  Object.values(process.env).some(value => value?.includes(token))
) process.exit(9);
process.stdout.write(JSON.stringify({ wire_version: 7, event: "dispatch_started" }) + "\n");
process.stdout.write(JSON.stringify({
  wire_version: 7,
  event: "completed",
  result: {
    content: [{ type: "text", text: `server echoed ${token}` }],
    structured_content: { present: true, value: { echoed: token } },
    is_error: false
  }
}) + "\n");
"#,
    );
    let selected = selected_tool("https://example.com/mcp");
    let authorization = McpCredentialHeader::for_test("fixture-secret-token");

    let result = call_tool(
        &adapter,
        &selected,
        Some(&authorization),
        &json!({"text": "hello"}),
        CancellationToken::new(),
    )
    .await
    .expect("authenticated call completes");

    assert_eq!(
        result.content,
        vec![renoa_agent::ContentBlock::text("server echoed [REDACTED]")]
    );
    assert_eq!(result.details, Some(json!({"echoed": "[REDACTED]"})));
}

fn assert_result(result: &McpCallResult) {
    assert_eq!(
        result.content,
        vec![renoa_agent::ContentBlock::text("hello")]
    );
    assert_eq!(result.details, Some(json!({"echoed": true})));
    assert!(!result.is_error);
}

fn selected_tool(endpoint: &str) -> ResolvedMcpTool {
    ResolvedMcpTool {
        integration_id: "fixture".to_owned(),
        connection_id: "primary".to_owned(),
        endpoint: endpoint.to_owned(),
        request_headers: crate::mcp::McpRequestHeaders::default(),
        protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
        adapter_revision: MCP_ADAPTER_REVISION.to_owned(),
        auth: McpConnectionAuth::None,
        tool: McpCatalogTool {
            name: "echo".to_owned(),
            description: "Echo one string.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            }),
            model_input_schema: json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            }),
            output_schema: None,
        },
    }
}

fn write_adapter(directory: &Path, source: &str) -> std::path::PathBuf {
    let adapter = directory.join("adapter.mjs");
    fs::write(&adapter, source).expect("write fake MCP adapter");
    adapter
}

async fn wait_for_path(path: &Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("adapter did not reach dispatch");
}
