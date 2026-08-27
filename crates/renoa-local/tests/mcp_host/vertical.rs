use std::{
    fs,
    io::Write as _,
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::Arc,
    thread,
    time::Duration,
};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock, Message};
use renoa_kernel::{EffectRecovery, Kernel, SessionId};
use renoa_local::{LocalHost, LocalTurnOutcome, ModelProvider};
use serde_json::{Value, json};
use tempfile::tempdir;
use uuid::Uuid;

use super::{compiled_adapter, read_http_request, workspace_root};

#[path = "vertical/model.rs"]
mod model;

use self::model::{read_json_lines, write_model_bridge};

#[tokio::test]
async fn selected_mcp_tool_runs_through_alpha_and_is_not_replayed_after_restart() {
    let repository = workspace_root();
    let adapter = compiled_adapter(&repository);
    let directory = tempdir().expect("temporary directory");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("model-bridge.mjs");
    let credentials = directory.path().join("credentials.sqlite3");
    let model_requests = directory.path().join("model-requests.jsonl");
    fs::create_dir(&workspace).expect("create Alpha workspace");
    fs::write(&credentials, "").expect("create credential placeholder");
    write_model_bridge(&bridge, &model_requests);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind MCP fixture");
    let address = listener.local_addr().expect("MCP fixture address");
    let endpoint = format!("http://127.0.0.1:{}/mcp", address.port());
    let server = thread::spawn(move || serve_vertical_mcp(&listener));
    let host = new_vertical_host(&data, &bridge, &credentials, &adapter);
    configure_echo_mcp(&host, &endpoint).await;

    let session = host
        .create_alpha_session(&workspace)
        .await
        .expect("create composed Alpha session");
    let session_id = session.id();
    let request_id = Uuid::new_v4();
    let prompt = vec![ContentBlock::text("Use the echo tool.")];
    let outcome = execute_tool_turn(&session, request_id, prompt.clone(), address).await;
    assert_eq!(
        outcome,
        LocalTurnOutcome::Completed {
            output: "Echo completed.".to_owned(),
            stop_reason: renoa_agent::StopReason::Stop,
        }
    );
    let denied_request_id = Uuid::new_v4();
    let denied_outcome = execute_tool_turn(
        &session,
        denied_request_id,
        vec![ContentBlock::text("Use the denied echo tool.")],
        address,
    )
    .await;
    assert_eq!(
        denied_outcome,
        LocalTurnOutcome::Completed {
            output: "MCP error handled.".to_owned(),
            stop_reason: renoa_agent::StopReason::Stop,
        }
    );
    let lost_request_id = Uuid::new_v4();
    let lost_prompt = vec![ContentBlock::text("Use the lost echo tool.")];
    let lost_outcome =
        execute_tool_turn(&session, lost_request_id, lost_prompt.clone(), address).await;
    assert_eq!(
        lost_outcome,
        LocalTurnOutcome::Failed {
            reason: "effect outcome is unknown; operation was abandoned without replay".to_owned(),
        }
    );

    let methods = server.join().expect("MCP fixture thread");
    assert_eq!(
        methods,
        [
            "server/discover",
            "tools/list",
            "server/discover",
            "tools/call",
            "server/discover",
            "tools/call",
            "server/discover",
            "tools/call",
        ]
    );
    let replayed_lost = session
        .execute_turn(lost_request_id, lost_prompt, Arc::new(NoopEvents))
        .await
        .expect("replay the abandoned outcome without another tool call");
    assert_eq!(replayed_lost, lost_outcome);
    assert_model_context(&model_requests);
    assert_durable_tool_result(&session.history().expect("load durable history"));

    drop(session);
    drop(host);
    assert_frozen_mcp_binding(&data, session_id);
    let reopened = new_vertical_host(&data, &bridge, &credentials, &adapter);
    let restored = reopened
        .load_alpha_session(session_id, &workspace)
        .await
        .expect("restore exact Alpha session");
    let replayed = restored
        .execute_turn(request_id, prompt, Arc::new(NoopEvents))
        .await
        .expect("replay settled command from durable history");

    assert_eq!(replayed, outcome);
    assert_eq!(read_json_lines(&model_requests).len(), 5);
    assert_durable_tool_result(&restored.history().expect("reload durable history"));
}

async fn configure_echo_mcp(host: &LocalHost, endpoint: &str) {
    host.register_direct_mcp_connection("fixture", "primary", endpoint)
        .await
        .expect("register MCP integration");
    let catalog = host
        .refresh_mcp_catalog("primary")
        .await
        .expect("discover real MCP catalog");
    assert_eq!(
        catalog
            .tools()
            .iter()
            .map(renoa_local::McpCatalogTool::name)
            .collect::<Vec<_>>(),
        ["echo", "unused"]
    );
    host.select_alpha_mcp_tool("primary", "echo")
        .await
        .expect("select only echo for Alpha");
}

async fn execute_tool_turn(
    session: &renoa_local::AlphaSession,
    request_id: Uuid,
    prompt: Vec<ContentBlock>,
    server: SocketAddr,
) -> LocalTurnOutcome {
    let outcome = session
        .execute_turn(request_id, prompt, Arc::new(NoopEvents))
        .await;
    if outcome.is_err() {
        let _ignored = TcpStream::connect(server);
    }
    outcome.expect("run Alpha through MCP and the kernel")
}

fn new_vertical_host(data: &Path, bridge: &Path, credentials: &Path, adapter: &Path) -> LocalHost {
    LocalHost::new(
        data,
        bridge,
        vec![ModelProvider::Xai],
        ModelProvider::Xai,
        "fixture-model",
        credentials,
        Some(adapter),
    )
    .expect("create local Host")
}

fn assert_model_context(path: &Path) {
    let requests = read_json_lines(path);
    assert_eq!(requests.len(), 5);
    for request in &requests {
        let tools = request["tools"].as_array().expect("model tools array");
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool["name"].as_str().expect("tool name"))
                .collect::<Vec<_>>(),
            [
                "read_file",
                "edit_file",
                "write_file",
                "bash",
                "grep",
                "find",
                "echo",
            ]
        );
        let echo = tools.last().expect("selected echo schema");
        assert_eq!(echo["description"], "Echo one string.");
        assert_eq!(echo["input_schema"]["required"], json!(["tenant", "text"]));
        assert!(
            echo["input_schema"]["properties"]["tenant"]
                .get("x-mcp-header")
                .is_none(),
            "transport annotations must not enter model context"
        );
        let encoded = serde_json::to_string(echo).expect("encode model-visible tool");
        for forbidden in [
            "endpoint",
            "output_schema",
            "adapter_revision",
            "connection_id",
            "integration_id",
        ] {
            assert!(
                !encoded.contains(forbidden),
                "model tool leaked {forbidden}"
            );
        }
        assert!(tools.iter().all(|tool| tool["name"] != "unused"));
    }
}

fn assert_durable_tool_result(history: &[renoa_local::LocalHistoryEntry]) {
    assert_eq!(
        history.len(),
        11,
        "durable replay must not duplicate history"
    );
    let Message::Tool { result } = &history[2].message else {
        panic!("third durable message must be the MCP result")
    };
    assert_eq!(result.name, "echo");
    assert_eq!(result.call_id, "mcp-echo-1");
    assert_eq!(result.content, vec![ContentBlock::text("echo: hello")]);
    assert_eq!(result.details, Some(json!({"echoed": "hello"})));
    assert!(!result.is_error);
    let Message::Tool { result } = &history[6].message else {
        panic!("seventh durable message must be the MCP error result")
    };
    assert_eq!(result.name, "echo");
    assert_eq!(result.call_id, "mcp-echo-denied");
    assert_eq!(
        result.content,
        vec![ContentBlock::text("permission denied")]
    );
    assert_eq!(result.details, Some(json!({"echoed": "denied"})));
    assert!(result.is_error);
    let Message::Tool { result } = &history[10].message else {
        panic!("unknown MCP outcome must leave balanced durable tool history")
    };
    assert_eq!(result.name, "echo");
    assert_eq!(result.call_id, "mcp-echo-lost");
    assert_eq!(
        result.content,
        vec![ContentBlock::text(
            "This tool may have finished, but Renoa could not recover its result. It was not run again."
        )]
    );
    assert!(result.is_error);
}

fn assert_frozen_mcp_binding(data: &Path, session_uuid: Uuid) {
    let database = data
        .join("sessions")
        .join(session_uuid.to_string())
        .join("kernel.sqlite3");
    let kernel = Kernel::open(database).expect("open persisted kernel");
    let snapshot = kernel
        .inspect(SessionId::from_uuid(session_uuid))
        .expect("inspect persisted operation");
    assert_eq!(snapshot.operations.len(), 3);
    let operation = snapshot.operations.first().expect("first operation");
    let manifest = operation
        .manifest
        .as_ref()
        .expect("frozen runtime manifest");
    let revision = manifest
        .effect_bindings
        .get("renoa.agent.tool/echo")
        .expect("frozen MCP effect binding");
    assert!(revision.starts_with("renoa-mcp-tool/v1/"));
    let effect = operation
        .effects
        .iter()
        .find(|effect| effect.binding == "renoa.agent.tool/echo")
        .expect("durable MCP effect");
    assert_eq!(effect.recovery, EffectRecovery::NeverReplay);
    assert_eq!(effect.dispatch_count, 1);
    assert_eq!(effect.binding_revision, *revision);
    for operation in &snapshot.operations {
        let effect = operation
            .effects
            .iter()
            .find(|effect| effect.binding == "renoa.agent.tool/echo")
            .expect("each operation has one durable MCP effect");
        assert_eq!(effect.recovery, EffectRecovery::NeverReplay);
        assert_eq!(effect.dispatch_count, 1);
    }
}

fn serve_vertical_mcp(listener: &TcpListener) -> Vec<String> {
    let expected = [
        "server/discover",
        "tools/list",
        "server/discover",
        "tools/call",
        "server/discover",
        "tools/call",
        "server/discover",
        "tools/call",
    ];
    let mut methods = Vec::new();
    for expected_method in expected {
        let (mut stream, _) = listener.accept().expect("accept MCP request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("configure MCP request timeout");
        let request = read_http_request(&mut stream).expect("read MCP request");
        let header_end = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("MCP HTTP body");
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let body = &request[header_end + 4..];
        let rpc: Value = serde_json::from_slice(body).expect("decode MCP JSON-RPC request");
        let method = rpc["method"].as_str().expect("MCP method");
        assert_eq!(method, expected_method);
        methods.push(method.to_owned());
        let result = match method {
            "server/discover" => json!({
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}}
            }),
            "tools/list" => json!({
                "resultType": "complete",
                "tools": [
                    {
                        "name": "unused",
                        "description": "Must stay outside model context.",
                        "inputSchema": {"type": "object", "properties": {}}
                    },
                    {
                        "name": "echo",
                        "description": "Echo one string.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "tenant": {"type": "string", "x-mcp-header": "Tenant"},
                                "text": {"type": "string"}
                            },
                            "required": ["tenant", "text"]
                        },
                        "outputSchema": {
                            "type": "object",
                            "properties": {"echoed": {"type": "string"}},
                            "required": ["echoed"]
                        }
                    }
                ],
                "ttlMs": 0,
                "cacheScope": "private"
            }),
            "tools/call" => match tool_call_result(&rpc, &headers) {
                Some(result) => result,
                None => continue,
            },
            _ => unreachable!("expected methods are closed"),
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

fn tool_call_result(rpc: &Value, headers: &str) -> Option<Value> {
    assert_eq!(rpc["params"]["name"], "echo");
    assert_eq!(rpc["params"]["arguments"]["tenant"], "alpha");
    let text = rpc["params"]["arguments"]["text"]
        .as_str()
        .expect("echo text");
    assert!(matches!(text, "hello" | "denied" | "lost"));
    assert!(
        headers.lines().any(|line| line
            .trim_end_matches('\r')
            .eq_ignore_ascii_case("mcp-param-tenant: alpha")),
        "frozen raw schema did not project its routing header"
    );
    if text == "lost" {
        return None;
    }
    Some(json!({
        "resultType": "complete",
        "content": [{
            "type": "text",
            "text": if text == "hello" { "echo: hello" } else { "permission denied" }
        }],
        "structuredContent": {"echoed": text},
        "isError": text == "denied"
    }))
}

struct NoopEvents;

impl AgentEventSink for NoopEvents {
    fn emit(&self, _event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}
