use std::{
    collections::BTreeSet,
    fs,
    io::Write as _,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

use renoa_agent::{ContentBlock, Message, StopReason};
use renoa_kernel::{EffectOutcome, EffectRecovery, Kernel, SessionId};
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalTurnOutcome,
};
use serde_json::{Value, json};
use tempfile::tempdir;
use uuid::Uuid;

use super::{
    ALPHA_PRESET_ID, NoopEvents, compiled_adapter, configure_echo_mcp, execute_tool_turn,
    model::{read_json_lines, write_model_bridge_with_behavior},
    new_vertical_host, read_http_request, tool_call_result, vertical_tool_catalog, workspace_root,
};

#[tokio::test]
async fn code_mode_gathers_real_mcp_calls_into_one_durable_model_result() {
    let Some(worker) = std::env::var_os("RENOA_TEST_MONTY_WORKER").map(PathBuf::from) else {
        return;
    };
    renoa_local::validate_code_mode_worker(&worker).expect("verified pinned Monty worker");
    let adapter = compiled_adapter(&workspace_root());
    let directory = tempdir().expect("temporary directory");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("model-bridge.mjs");
    let credentials = directory.path().join("credentials.sqlite3");
    let model_requests = directory.path().join("model-requests.jsonl");
    fs::create_dir(&workspace).expect("create Alpha workspace");
    fs::write(&credentials, "").expect("create credential placeholder");
    write_model_bridge_with_behavior(
        &bridge,
        &model_requests,
        include_str!("code_mode_model.mjs"),
    );

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind MCP fixture");
    let address = listener.local_addr().expect("MCP fixture address");
    let endpoint = format!("http://127.0.0.1:{}/mcp", address.port());
    let server = thread::spawn(move || serve_code_mode_mcp(&listener));
    let host = new_vertical_host(&data, &bridge, &credentials, &adapter, Some(&worker));
    let request = AgentCreateRequest::new(
        Uuid::new_v4(),
        AgentPresetId::new(ALPHA_PRESET_ID).expect("Alpha preset"),
        "Alpha",
    )
    .with_tools(["code_mode".to_owned()]);
    let alpha = host
        .create_agent(
            AgentCreator::System {
                component: "code-mode-host-test".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            request,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("create Code Mode Alpha")
        .id;
    let selection = &host
        .agent_definition(alpha)
        .await
        .expect("read Code Mode Alpha")
        .expect("Alpha exists")
        .tool_selection
        .tools;
    assert!(selection.contains("code_mode"));
    assert!(selection.contains("tool_execute"));
    configure_echo_mcp(&host, &endpoint, alpha).await;

    let session = host
        .ensure_agent_session(alpha, &workspace, Uuid::new_v4())
        .await
        .expect("create composed Code Mode session");
    let session_id = session.id();
    let request_id = Uuid::new_v4();
    let prompt = vec![ContentBlock::text("Use Code Mode for both echo calls.")];
    let outcome = execute_tool_turn(&session, request_id, prompt.clone(), address).await;
    assert_eq!(
        outcome,
        LocalTurnOutcome::Completed {
            output: "Code Mode MCP results handled.".to_owned(),
            stop_reason: StopReason::Stop,
        }
    );
    let _ignored = TcpStream::connect(address);
    let (methods, calls) = server.join().expect("MCP fixture thread");
    assert_mcp_traffic(&methods, &calls);

    let history = session.history().expect("load durable history");
    assert_code_mode_history(&history);
    assert_model_context(&model_requests, &endpoint);

    drop(session);
    drop(host);
    assert_code_mode_effects(&data, session_id);
    let reopened = new_vertical_host(&data, &bridge, &credentials, &adapter, Some(&worker));
    let restored = reopened
        .load_session_for_agent(alpha, session_id, &workspace)
        .await
        .expect("restore exact Code Mode session");
    let replayed = restored
        .execute_turn(request_id, prompt, Arc::new(NoopEvents))
        .await
        .expect("replay settled Code Mode command");
    assert_eq!(replayed, outcome);
    assert_eq!(restored.history().expect("reloaded history"), history);
    assert_eq!(read_json_lines(&model_requests).len(), 4);
    drop(restored);
    drop(reopened);
    assert_code_mode_effects(&data, session_id);
}

fn assert_mcp_traffic(methods: &[String], calls: &[String]) {
    assert_eq!(methods.len(), 6);
    assert_eq!(methods[0..2], ["server/discover", "tools/list"]);
    assert_eq!(
        methods
            .iter()
            .filter(|method| *method == "server/discover")
            .count(),
        3
    );
    assert_eq!(
        methods
            .iter()
            .filter(|method| *method == "tools/call")
            .count(),
        2
    );
    assert_eq!(calls, ["denied", "hello"]);
}

fn assert_code_mode_history(history: &[renoa_local::LocalHistoryEntry]) {
    assert_eq!(history.len(), 8);
    let results = history
        .iter()
        .filter_map(|entry| match &entry.message {
            Message::Tool { result } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .map(|result| result.name.as_str())
            .collect::<Vec<_>>(),
        ["tool_search", "tool_load", "code_mode"]
    );
    let outer = results[2];
    assert_eq!(outer.call_id, "code-mode-outer");
    assert!(!outer.is_error);
    assert_eq!(outer.details, None);
    let [ContentBlock::Text { text }] = outer.content.as_slice() else {
        panic!("Code Mode must produce one text result")
    };
    let value: Value = serde_json::from_str(text).expect("decode final Python value");
    assert_eq!(value[0], "echo: hello");
    assert_eq!(value[1], true);
    assert!(value[2].as_str().expect("error text").contains("HTTP 401"));
}

fn assert_model_context(path: &Path, endpoint: &str) {
    let requests = read_json_lines(path);
    assert_eq!(requests.len(), 4);
    for request in &requests {
        let names = request["tools"]
            .as_array()
            .expect("model tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect::<Vec<_>>();
        assert!(names.contains(&"tool_search"));
        assert!(names.contains(&"tool_load"));
        assert!(names.contains(&"code_mode"));
        assert!(!names.contains(&"tool_execute"));
        assert!(!request.to_string().contains(endpoint));
    }
    let tool_results = requests[3]["messages"]
        .as_array()
        .expect("final model history")
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["result"]["name"].as_str().expect("result name"))
        .collect::<Vec<_>>();
    assert_eq!(tool_results, ["tool_search", "tool_load", "code_mode"]);
}

fn assert_code_mode_effects(data: &Path, session_uuid: Uuid) {
    let kernel = Kernel::open(
        data.join("sessions")
            .join(session_uuid.to_string())
            .join("kernel.sqlite3"),
    )
    .expect("open persisted kernel");
    let snapshot = kernel
        .inspect(SessionId::from_uuid(session_uuid))
        .expect("inspect Code Mode operation");
    assert_eq!(snapshot.operations.len(), 1);
    let operation = &snapshot.operations[0];
    let manifest = operation.manifest.as_ref().expect("frozen runtime");
    let revision = manifest
        .effect_bindings
        .get("renoa.agent.tool/tool_execute")
        .expect("hidden MCP executor is frozen");
    assert!(
        manifest
            .effect_bindings
            .contains_key("renoa.agent.code-mode.step")
    );
    let waves = operation
        .effect_batches
        .iter()
        .filter(|batch| {
            batch
                .effects
                .iter()
                .any(|effect| effect.binding == "renoa.agent.tool/tool_execute")
        })
        .collect::<Vec<_>>();
    assert_eq!(waves.len(), 1);
    let effects = &waves[0].effects;
    assert_eq!(effects.len(), 2);
    assert_ne!(effects[0].effect_id, effects[1].effect_id);
    let mut arguments = BTreeSet::new();
    let mut identities = BTreeSet::new();
    for effect in effects {
        assert_eq!(effect.binding, "renoa.agent.tool/tool_execute");
        assert_eq!(effect.binding_revision, *revision);
        assert_eq!(effect.recovery, EffectRecovery::NeverReplay);
        assert_eq!(effect.dispatch_count, 1);
        assert_eq!(effect.request["name"], "tool_execute");
        identities.insert(effect.request["id"].as_str().expect("nested call identity"));
        arguments.insert(
            effect.request["arguments"]["arguments"]["text"]
                .as_str()
                .expect("nested echo argument"),
        );
        let Some(EffectOutcome::Success(value)) = &effect.outcome else {
            panic!("nested MCP effect did not settle")
        };
        assert_eq!(value["name"], "tool_execute");
    }
    assert_eq!(identities.len(), 2);
    assert_eq!(arguments, BTreeSet::from(["denied", "hello"]));
    assert_eq!(
        effects
            .iter()
            .map(|effect| {
                let Some(EffectOutcome::Success(value)) = &effect.outcome else {
                    unreachable!("checked above")
                };
                (
                    effect.request["arguments"]["arguments"]["text"]
                        .as_str()
                        .expect("echo argument"),
                    value["is_error"].as_bool().expect("MCP error flag"),
                )
            })
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([("denied", true), ("hello", false)])
    );
}

fn serve_code_mode_mcp(listener: &TcpListener) -> (Vec<String>, Vec<String>) {
    let mut methods = Vec::new();
    let mut calls = Vec::new();
    for _ in 0..6 {
        let (mut stream, _) = listener.accept().expect("accept MCP request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("configure MCP request timeout");
        let request = read_http_request(&mut stream).expect("read MCP request");
        if request.is_empty() {
            break;
        }
        let header_end = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("MCP HTTP body");
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let rpc: Value = serde_json::from_slice(&request[header_end + 4..])
            .expect("decode MCP JSON-RPC request");
        let method = rpc["method"].as_str().expect("MCP method");
        methods.push(method.to_owned());
        let (status, result) = match method {
            "server/discover" => (
                200,
                json!({
                    "resultType": "complete",
                    "supportedVersions": ["2026-07-28"],
                    "capabilities": {"tools": {}}
                }),
            ),
            "tools/list" => (200, vertical_tool_catalog()),
            "tools/call" => {
                calls.push(
                    rpc["params"]["arguments"]["text"]
                        .as_str()
                        .expect("echo text")
                        .to_owned(),
                );
                tool_call_result(&rpc, &headers).expect("MCP call has a definite response")
            }
            _ => panic!("unexpected MCP method: {method}"),
        };
        let response = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": rpc["id"],
            "result": result,
        }))
        .expect("encode MCP response");
        write!(
            stream,
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            if status == 200 { "OK" } else { "Unauthorized" },
            response.len(),
        )
        .expect("write MCP response headers");
        stream.write_all(&response).expect("write MCP response");
        stream.flush().expect("flush MCP response");
    }
    calls.sort();
    (methods, calls)
}
