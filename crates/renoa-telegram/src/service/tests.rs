use std::{collections::HashMap, fs, sync::Arc};

use renoa_agent::Message;
use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, arcee_profile,
};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

use super::{ActiveTurn, Worker, retry_delay};
use crate::{
    api::{ApiError, TelegramApi},
    ingress::{InboundKind, ParsedUpdate, Topic},
    store::{PendingAction, SurfaceStore},
};

#[test]
fn polling_backoff_is_bounded_and_honors_server_delay() {
    let network = ApiError::Transport {
        method: "getUpdates",
        category: "connection",
        detail: "connection reset".to_owned(),
    };
    assert_eq!(retry_delay(&network, 1).as_secs(), 1);
    assert_eq!(retry_delay(&network, 99).as_secs(), 32);
    let remote = ApiError::Remote {
        method: "getUpdates",
        code: 429,
        description: "slow down".to_owned(),
        retry_after: Some(90),
    };
    assert_eq!(retry_delay(&remote, 1).as_secs(), 60);
}

#[tokio::test]
async fn one_telegram_prompt_crosses_the_real_arcee_host_and_kernel_path() {
    let directory = tempdir().expect("temporary service root");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("model-bridge.mjs");
    let credentials = directory.path().join("credentials.sqlite3");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&bridge, MODEL_BRIDGE).expect("write deterministic bridge");
    fs::write(&credentials, "").expect("write credential placeholder");
    let profile = arcee_profile(&data).expect("create Arcee profile");
    let profile_id = profile.id().clone();
    let host = LocalHost::new(
        &data,
        LocalModelConfiguration::new(
            &bridge,
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "fixture-model",
            &credentials,
        ),
        vec![profile],
        LocalHostAdapters::default(),
    )
    .expect("assemble Arcee Host");
    let store = SurfaceStore::open(&data).expect("open Telegram store");
    store
        .bind_identity(9, 42, &workspace)
        .await
        .expect("bind Telegram identity");
    store
        .admit(ParsedUpdate {
            update_id: 1,
            canonical: b"telegram-update-1".to_vec(),
            topic: Some(Topic {
                chat_id: 42,
                thread_id: None,
            }),
            message_id: Some(10),
            kind: InboundKind::Prompt("Do the real task.".to_owned()),
        })
        .await
        .expect("admit Telegram prompt");
    let PendingAction::Execute(work) = store
        .next_action()
        .await
        .expect("load work")
        .expect("queued work")
    else {
        panic!("prompt did not become executable");
    };
    let session_id = work.session_id;
    let (origin, server_shutdown, server) = draft_server().await;
    let mut worker = Worker {
        api: Arc::new(TelegramApi::for_test(&origin, "9:test").expect("test API")),
        store: store.clone(),
        host: Arc::new(host),
        profile_id,
        workspace,
        sessions: HashMap::new(),
        active: Arc::new(ActiveTurn::default()),
        wake: Arc::new(tokio::sync::Notify::new()),
        shutdown: CancellationToken::new(),
    };

    worker.execute(work).await.expect("execute Arcee prompt");
    let PendingAction::Deliver(delivery) = store
        .next_action()
        .await
        .expect("load result")
        .expect("ready result")
    else {
        panic!("Arcee result did not become deliverable");
    };
    assert_eq!(delivery.text, "Arcee completed the real path.");
    let history = worker
        .sessions
        .get(&session_id)
        .expect("cached Arcee session")
        .history()
        .expect("durable Arcee history");
    assert!(history.len() >= 2);
    assert_eq!(history[0].message, Message::user_text("Do the real task."));

    server_shutdown.cancel();
    server.await.expect("draft server task");
}

async fn draft_server() -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind draft server");
    let origin = format!("http://{}", listener.local_addr().expect("server address"));
    let shutdown = CancellationToken::new();
    let task_shutdown = shutdown.clone();
    let task = tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                () = task_shutdown.cancelled() => return,
                accepted = listener.accept() => accepted,
            };
            let (mut stream, _) = accepted.expect("accept draft request");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).await.expect("read draft request");
            let body = r#"{"ok":true,"result":true}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write draft response");
        }
    });
    (origin, shutdown, task)
}

const MODEL_BRIDGE: &str = r#"
import { createHash } from "node:crypto";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const action = process.env.RENOA_MODEL_ACTION;
const modelSpec = JSON.stringify({ id: "fixture-model" });
if (action === "catalog") {
  process.stdout.write(JSON.stringify({ ok: true, response: { models: [{
    id: "fixture-model",
    name: "Fixture Model",
    reasoning_levels: ["high"],
    context_window_tokens: 1000000,
    model_spec: { id: "fixture-model" }
  }] } }));
  process.exit(0);
}
if (action === "describe") {
  process.stdout.write(JSON.stringify({ ok: true, response: {
    context_window_tokens: 1000000,
    max_output_tokens: 8192,
    model_spec: modelSpec,
    model_binding_id: createHash("sha256").update(modelSpec).digest("hex"),
    reasoning_level: "high"
  } }));
  process.exit(0);
}
if (action !== "stream") process.exit(2);
const request = JSON.parse(input);
if (!request.system_prompt.startsWith("You are Arcee, Renoa's personal operator.")) {
  process.stderr.write("Telegram surface selected the wrong profile");
  process.exit(3);
}
if (!request.tools.some((tool) => tool.name === "profile_update")) {
  process.stderr.write("Arcee profile update tool was not assembled");
  process.exit(4);
}
if (request.system_prompt.includes("current_time:")) {
  process.stderr.write("dynamic time polluted Arcee's stable system prompt");
  process.exit(5);
}
const user = request.messages.at(-1);
if (user?.role !== "user" || user.content.length !== 2 ||
    !user.content[1]?.text?.includes("<turn_context>\ncurrent_time:")) {
  process.stderr.write("Telegram receive time did not reach Arcee's model turn");
  process.exit(6);
}
process.stdout.write(JSON.stringify({
  event: "completed",
  response: {
    content: [{ type: "text", text: "Arcee completed the real path." }],
    stop_reason: "stop",
    usage: { input: 8, output: 4, cache_read: 0, cache_write: 0 },
    metadata: { api: "test", provider: "xai", model: "fixture-model" }
  }
}) + "\n");
"#;
