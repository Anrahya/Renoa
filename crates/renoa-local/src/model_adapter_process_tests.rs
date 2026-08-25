use std::{
    fs,
    io::{Read as _, Write as _},
    net::{Shutdown, TcpListener, TcpStream},
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use renoa_agent::{AssistantContent, ContentBlock, ModelRequest, StopReason, sample_model};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, ContextBinding, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command as KernelCommand, CommandId, DriveResult, EffectRecovery, EffectStatus,
    Kernel, OperationStatus, SessionId,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use crate::model_bridge::BridgeModel;

const SSE: &str = concat!(
    "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1,",
    "\"model\":\"grok-4.6\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",",
    "\"content\":\"from-compiled-adapter\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1,",
    "\"model\":\"grok-4.6\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],",
    "\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4,\"total_tokens\":16,",
    "\"prompt_tokens_details\":{\"cached_tokens\":2,\"cache_write_tokens\":1}}}\n\n",
    "data: [DONE]\n\n",
);

#[tokio::test]
async fn rust_launches_the_compiled_adapter_and_consumes_its_protocol() {
    let workspace = workspace_root();
    let adapter = compiled_adapter(&workspace);
    let catalog = workspace.join("adapters/model-provider-node/src/upstream/catalogs/xai.json");
    let directory = tempdir().expect("temporary directory");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider");
    let address = listener.local_addr().expect("fake provider address");
    let server = thread::spawn(move || serve_one_chat_completion(&listener));

    let mut model = load_catalog_model(&catalog, "grok-4.6");
    model["baseUrl"] = json!(format!("http://127.0.0.1:{}/v1", address.port()));
    let spec = serde_json::to_string(&model).expect("encode loopback model spec");

    let auth_store = directory.path().join("pi-auth.sqlite");
    write_oauth_store(&auth_store);
    let trampoline = write_loopback_trampoline(directory.path(), &adapter);

    let model = BridgeModel::load_with_spec(
        &trampoline,
        "xai",
        "grok-4.6",
        &auth_store,
        Some(spec),
        None,
        NonZeroU32::new(32_768).expect("non-zero output cap"),
    )
    .await
    .expect("compiled adapter describe");

    let sampled = sample_model(
        &model,
        ModelRequest {
            system_prompt: "Be precise.".to_owned(),
            messages: vec![renoa_agent::Message::User {
                content: vec![ContentBlock::text("Hello")],
            }],
            tools: Vec::new(),
        },
        CancellationToken::new(),
        None,
    )
    .await;
    let _ = std::net::TcpStream::connect(address);
    server.join().expect("fake provider thread");
    let sampled = sampled.expect("compiled adapter stream");

    assert_eq!(
        sampled.response.content,
        vec![AssistantContent::text("from-compiled-adapter")]
    );
    assert_eq!(sampled.response.stop_reason, StopReason::Stop);
    assert_eq!(sampled.response.metadata.provider.as_deref(), Some("xai"));
    assert_eq!(sampled.response.metadata.model.as_deref(), Some("grok-4.6"));
}

#[tokio::test]
async fn post_dispatch_socket_reset_never_settles_a_definite_kernel_failure() {
    let workspace = workspace_root();
    let adapter = compiled_adapter(&workspace);
    let catalog = workspace.join("adapters/model-provider-node/src/upstream/catalogs/xai.json");
    let directory = tempdir().expect("temporary directory");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider");
    let address = listener.local_addr().expect("fake provider address");
    let received = Arc::new(Mutex::new(Vec::new()));
    let server_received = Arc::clone(&received);
    let server =
        thread::spawn(move || serve_reset_after_complete_request(&listener, &server_received));

    let mut model = load_catalog_model(&catalog, "grok-4.6");
    model["baseUrl"] = json!(format!("http://127.0.0.1:{}/v1", address.port()));
    let spec = serde_json::to_string(&model).expect("encode loopback model spec");
    let auth_store = directory.path().join("pi-auth.sqlite");
    write_oauth_store(&auth_store);
    let trampoline = write_loopback_trampoline(directory.path(), &adapter);
    let model = Arc::new(
        BridgeModel::load_with_spec(
            &trampoline,
            "xai",
            "grok-4.6",
            &auth_store,
            Some(spec),
            None,
            NonZeroU32::new(32_768).expect("non-zero output cap"),
        )
        .await
        .expect("compiled adapter describe"),
    );
    let revision = format!(
        "renoa-model-provider-node/v1/xai/grok-4.6/{}/reasoning-{}",
        model.binding_id(),
        model.reasoning().as_str()
    );
    let runtime = build_runtime(
        AgentLoopConfig::new(
            "Classify this model result honestly.",
            NonZeroU32::new(4).expect("non-zero model limit"),
            NonZeroU32::new(4).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new(revision, model, EffectRecovery::SafeToReplay),
        Vec::new(),
    )
    .expect("build runtime");

    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let content = serde_json::to_value(AgentCommand::text("Transmit this request."))
        .expect("serialize command");
    kernel
        .submit(session_id, KernelCommand::new(CommandId::new(), content))
        .expect("submit command");

    let result = kernel
        .drive(session_id, &runtime)
        .await
        .expect("drive post-dispatch reset");
    let _ = std::net::TcpStream::connect(address);
    server.join().expect("fake provider thread");

    assert!(
        matches!(result, DriveResult::Blocked { .. }),
        "post-dispatch reset must not settle a definite outcome: {result:?}"
    );
    let snapshot = kernel.inspect(session_id).expect("inspect blocked turn");
    assert_eq!(
        snapshot.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(snapshot.operations[0].effects[0].outcome, None);
    assert_complete_chat_requests(&received.lock().expect("request lock"));
}

#[test]
fn compiled_adapter_classifies_malformed_requests_before_credentials_and_nonzero_exit() {
    let workspace = workspace_root();
    let adapter = compiled_adapter(&workspace);
    let mut child = Command::new("node")
        .args(["--dns-result-order=ipv4first"])
        .arg(&adapter)
        .env("RENOA_MODEL_ACTION", "stream")
        .env("RENOA_MODEL_PROVIDER", "xai")
        .env("RENOA_MODEL", "grok-4.6")
        .env(
            "RENOA_MODEL_AUTH_STORE",
            workspace.join("this-auth-store-must-not-be-opened.sqlite"),
        )
        .env("RENOA_MODEL_MAX_OUTPUT_TOKENS", "128")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn compiled adapter");
    {
        let stdin = child.stdin.as_mut().expect("adapter stdin");
        stdin
            .write_all(
                br#"{"system_prompt":1,"messages":[{"role":"user","content":"nope"}],"tools":[]}"#,
            )
            .expect("write malformed request");
    }
    let output = child.wait_with_output().expect("wait compiled adapter");
    assert!(
        !output.status.success(),
        "malformed request must exit nonzero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("TypeError"),
        "malformed request must not throw TypeError: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let record: Value = serde_json::from_str(stdout.lines().next().expect("stream error record"))
        .expect("decode stream error");
    assert_eq!(record["event"], "error");
    assert_eq!(record["error_kind"], "invalid_request");
    let message = record["error"].as_str().unwrap_or_default();
    assert!(
        !message.contains("credential"),
        "validation must run before loading credentials: {message}"
    );
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/renoa-local is two levels below the workspace")
        .to_path_buf()
}

fn compiled_adapter(workspace: &Path) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let adapter = workspace.join("adapters/model-provider-node/dist/src/main.js");
            let adapter_root = workspace.join("adapters/model-provider-node");
            let status = Command::new("pnpm")
                .args([
                    "--dir",
                    adapter_root.to_str().expect("adapter path is UTF-8"),
                    "build",
                ])
                .status()
                .expect("build @renoa/model-provider");
            assert!(
                status.success(),
                "adapter TypeScript build failed: {status}"
            );
            assert!(
                adapter.is_file(),
                "adapter build did not produce {}",
                adapter.display()
            );
            adapter
        })
        .clone()
}

fn write_loopback_trampoline(directory: &Path, adapter: &Path) -> PathBuf {
    let adapter = fs::canonicalize(adapter).expect("canonicalize compiled adapter");
    let url = format!("file://{}", adapter.display());
    let trampoline = directory.join("adapter.mjs");
    fs::write(
        &trampoline,
        format!(
            "process.env.RENOA_MODEL_ALLOW_LOOPBACK = '1';\nawait import({url});\n",
            url = serde_json::to_string(&url).expect("encode adapter URL")
        ),
    )
    .expect("write adapter trampoline");
    trampoline
}

fn write_oauth_store(path: &Path) {
    let database = Connection::open(path).expect("create credential database");
    database
        .execute_batch(
            "
            PRAGMA user_version = 1;
            CREATE TABLE credentials (
              provider_id TEXT PRIMARY KEY,
              credential_type TEXT NOT NULL CHECK (credential_type IN ('api_key', 'oauth')),
              credential_json TEXT NOT NULL
            ) STRICT;
            ",
        )
        .expect("create credential schema");
    database
        .execute(
            "INSERT INTO credentials (provider_id, credential_type, credential_json) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "xai",
                "oauth",
                r#"{"type":"oauth","access":"access-token-old","refresh":"refresh-token-old","expires":4000000000000}"#,
            ],
        )
        .expect("store OAuth credential");
}

fn load_catalog_model(path: &Path, model_id: &str) -> Value {
    let catalog: Value = serde_json::from_slice(&fs::read(path).expect("read pinned xAI catalog"))
        .expect("catalog JSON");
    catalog
        .get("openai-completions")
        .and_then(Value::as_object)
        .and_then(|models| models.get(model_id).cloned())
        .filter(Value::is_object)
        .expect("pinned grok-4.6 catalog entry")
}

fn assert_complete_chat_requests(requests: &[Vec<u8>]) {
    assert_eq!(requests.len(), 3);
    for request in requests {
        let text = String::from_utf8_lossy(request);
        assert!(text.starts_with("POST "), "method: {text}");
        assert!(
            text.contains("/chat/completions"),
            "chat completions route: {text}"
        );
        assert!(
            text.to_ascii_lowercase().contains("authorization: bearer "),
            "auth header: {text}"
        );
        let body = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|header_end| &request[header_end + 4..])
            .expect("HTTP body");
        let parsed: Value = serde_json::from_slice(body).expect("JSON body");
        assert_eq!(parsed["model"], "grok-4.6");
        assert!(parsed.get("messages").is_some());
    }
}

fn serve_reset_after_complete_request(listener: &TcpListener, received: &Mutex<Vec<Vec<u8>>>) {
    for _ in 0..3 {
        let (mut stream, _) = accept_with_timeout(listener, Duration::from_secs(5));
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("fake provider read timeout");
        let request = read_complete_http_request(&mut stream).expect("read complete request");
        received.lock().expect("request lock").push(request);
        let _ = stream.shutdown(Shutdown::Both);
    }
}

fn serve_one_chat_completion(listener: &TcpListener) {
    let (mut stream, _) = accept_with_timeout(listener, Duration::from_secs(5));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("fake provider read timeout");
    let _ = read_complete_http_request(&mut stream).expect("read provider request");
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{SSE}",
        SSE.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("write provider SSE");
}

fn read_complete_http_request(stream: &mut std::net::TcpStream) -> std::io::Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(data);
        }
        data.extend_from_slice(&buffer[..read]);
        let Some(header_end) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let content_length = std::str::from_utf8(&data[..header_end])
            .ok()
            .and_then(content_length)
            .unwrap_or(0);
        while data.len() < header_end + 4 + content_length {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                return Ok(data);
            }
            data.extend_from_slice(&buffer[..read]);
        }
        return Ok(data);
    }
}

fn accept_with_timeout(
    listener: &TcpListener,
    timeout: Duration,
) -> (TcpStream, std::net::SocketAddr) {
    listener
        .set_nonblocking(true)
        .expect("fake provider accept nonblocking");
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok(accepted) => {
                accepted
                    .0
                    .set_nonblocking(false)
                    .expect("fake provider stream blocking");
                return accepted;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    started.elapsed() < timeout,
                    "fake provider accept timed out after {timeout:?}"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("fake provider accept failed: {error}"),
        }
    }
}

fn content_length(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.eq_ignore_ascii_case("content-length"))
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}
