use std::{
    fs,
    io::Write as _,
    net::TcpListener,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    thread,
};

use renoa_agent::{
    AssistantContent, ContentBlock, Message, ModelRequest, StopReason, sample_model,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, ContextBinding, MESSAGE_EVENT_KIND, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command as KernelCommand, CommandId, DriveResult, EffectRecovery, EffectStatus,
    EventCursor, Kernel, OperationOutcome, OperationStatus, SessionId,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use crate::model_bridge::BridgeModel;

mod fake_provider;

use fake_provider::{
    assert_complete_chat_requests, serve_one_chat_completion, serve_reset_after_complete_request,
    serve_truncated_then_complete,
};

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

/// Builds a loopback runner over the compiled adapter and a fresh session with
/// one admitted text command, ready for a single `drive`.
async fn loopback_turn(
    directory: &Path,
    address: std::net::SocketAddr,
    system_prompt: &str,
) -> (Kernel, SessionId, renoa_kernel::Runtime) {
    let workspace = workspace_root();
    let adapter = compiled_adapter(&workspace);
    let catalog = workspace.join("adapters/model-provider-node/src/upstream/catalogs/xai.json");
    let mut model = load_catalog_model(&catalog, "grok-4.6");
    model["baseUrl"] = json!(format!("http://127.0.0.1:{}/v1", address.port()));
    let spec = serde_json::to_string(&model).expect("encode loopback model spec");
    let auth_store = directory.join("pi-auth.sqlite");
    write_oauth_store(&auth_store);
    let trampoline = write_loopback_trampoline(directory, &adapter);
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
            system_prompt,
            NonZeroU32::new(4).expect("non-zero model limit"),
            NonZeroU32::new(4).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new(revision, model, EffectRecovery::SafeToReplay),
        Vec::new(),
    )
    .expect("build runtime");

    let kernel = Kernel::open(directory.join("kernel.sqlite3")).expect("open kernel");
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
    (kernel, session_id, runtime)
}

#[tokio::test]
async fn post_dispatch_socket_reset_never_settles_a_definite_kernel_failure() {
    let directory = tempdir().expect("temporary directory");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider");
    let address = listener.local_addr().expect("fake provider address");
    let received = Arc::new(Mutex::new(Vec::new()));
    let server_received = Arc::clone(&received);
    let server =
        thread::spawn(move || serve_reset_after_complete_request(&listener, &server_received));

    let (kernel, session_id, runtime) = loopback_turn(
        directory.path(),
        address,
        "Classify this model result honestly.",
    )
    .await;
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
    assert_eq!(
        snapshot.operations[0].effects[0].dispatch_count, 2,
        "the live unknown outcome must replay once through the same effect"
    );
    // Each dispatch may spend its own transport retry budget, so the total
    // request count is adapter transport policy rather than a kernel fact. This
    // fixture accepts only the first dispatch's attempts, so these requests
    // prove the provider was reached before the unknown became durable; the
    // replay itself is proven by the dispatch count above.
    assert!(
        received.lock().expect("request lock").len() >= 2,
        "the provider must be reached before the effect becomes durably unknown"
    );
    assert_complete_chat_requests(&received.lock().expect("request lock"));
}

#[tokio::test]
async fn a_truncated_provider_stream_replays_the_model_effect_and_completes_the_turn() {
    let directory = tempdir().expect("temporary directory");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider");
    let address = listener.local_addr().expect("fake provider address");
    let received = Arc::new(Mutex::new(Vec::new()));
    let server_received = Arc::clone(&received);
    let server = thread::spawn(move || serve_truncated_then_complete(&listener, &server_received));

    let (kernel, session_id, runtime) = loopback_turn(
        directory.path(),
        address,
        "Replay one truncated provider stream.",
    )
    .await;
    let result = kernel
        .drive(session_id, &runtime)
        .await
        .expect("drive truncated stream");
    let _ = std::net::TcpStream::connect(address);
    server.join().expect("fake provider thread");

    assert!(
        matches!(
            result,
            DriveResult::Finished {
                outcome: OperationOutcome::Completed,
                ..
            }
        ),
        "a stream truncated after output must replay instead of failing the turn: {result:?}"
    );
    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect replayed truncated stream");
    assert_eq!(snapshot.operations[0].effects.len(), 1);
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::Settled
    );
    assert_eq!(snapshot.operations[0].effects[0].dispatch_count, 2);

    let received = received.lock().expect("request lock");
    assert_eq!(
        received.len(),
        2,
        "a stream truncated after exposed output is not transport-retryable, so each dispatch makes one request"
    );
    assert_complete_chat_requests(&received);
    drop(received);

    let messages: Vec<Message> = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable history")
        .events
        .into_iter()
        .filter(|event| event.kind == MESSAGE_EVENT_KIND)
        .map(|event| serde_json::from_value(event.payload).expect("decode durable message"))
        .collect();
    assert_eq!(
        messages.len(),
        2,
        "one user message and one settled assistant message: {messages:?}"
    );
    let Message::Assistant { content, .. } = &messages[1] else {
        panic!("expected one assistant message, found {:?}", messages[1]);
    };
    assert_eq!(content, &[AssistantContent::text("from-compiled-adapter")]);
    let durable = serde_json::to_string(&messages).expect("encode durable history");
    assert!(
        !durable.contains("discarded-partial-answer"),
        "the truncated attempt must leave no durable output: {durable}"
    );
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
