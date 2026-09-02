use std::{collections::HashMap, fs, sync::Arc};

use renoa_agent::Message;
use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, arcee_profile,
};
use tempfile::{TempDir, tempdir};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

use super::{ActiveTurn, Worker, retry_delay};
use crate::{
    api::{ApiError, TelegramApi},
    ingress::{InboundKind, ParsedUpdate, Topic},
    store::{DeliveryItem, PendingAction, SurfaceStore, WorkItem},
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
    let mut fixture = service_fixture().await;
    let work = admit_work(
        &fixture.store,
        1,
        InboundKind::Prompt("Do the real task.".to_owned()),
    )
    .await;
    let session_id = work.session_id;
    fixture
        .worker
        .execute(work)
        .await
        .expect("execute Arcee prompt");
    let delivery = ready_delivery(&fixture.store).await;
    assert_eq!(delivery.text, "Arcee completed the real path.");
    let history = fixture
        .worker
        .sessions
        .get(&session_id)
        .expect("cached Arcee session")
        .history()
        .expect("durable Arcee history");
    assert!(history.len() >= 2);
    assert_eq!(history[0].message, Message::user_text("Do the real task."));
    fixture.shutdown().await;
}

#[tokio::test]
async fn telegram_model_commands_use_the_surface_neutral_session_configuration() {
    let mut fixture = service_fixture().await;
    let model_work = admit_work(
        &fixture.store,
        1,
        InboundKind::Model(Some("fixture-model-b".to_owned())),
    )
    .await;
    let session_id = model_work.session_id;
    fixture
        .worker
        .execute(model_work)
        .await
        .expect("execute model command");
    let model_delivery = ready_delivery(&fixture.store).await;
    assert_eq!(
        model_delivery.text,
        "Model changed to Fixture Model B (fixture-model-b).\nReasoning: High."
    );
    assert_eq!(
        fixture
            .worker
            .sessions
            .get(&session_id)
            .expect("cached Arcee session")
            .configuration()
            .expect("selected configuration")
            .model,
        "opencode-go/fixture-model-b"
    );
    finish_delivery(&fixture.store, model_delivery, 81).await;
    let reasoning_work = admit_work(
        &fixture.store,
        2,
        InboundKind::Reasoning(Some("low".to_owned())),
    )
    .await;
    fixture
        .worker
        .execute(reasoning_work)
        .await
        .expect("execute reasoning command");
    let reasoning_delivery = ready_delivery(&fixture.store).await;
    assert_eq!(
        reasoning_delivery.text,
        "Reasoning changed to Low for Fixture Model B."
    );
    assert_eq!(
        fixture
            .worker
            .sessions
            .get(&session_id)
            .expect("cached Arcee session")
            .configuration()
            .expect("reasoning configuration")
            .reasoning,
        renoa_local::ReasoningLevel::Low
    );
    fixture.shutdown().await;
}

struct ServiceFixture {
    _directory: TempDir,
    store: SurfaceStore,
    worker: Worker,
    server_shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl ServiceFixture {
    async fn shutdown(self) {
        self.server_shutdown.cancel();
        self.server.await.expect("draft server task");
    }
}

async fn service_fixture() -> ServiceFixture {
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
            vec![ModelProvider::OpenCodeGo],
            ModelProvider::OpenCodeGo,
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
    let (origin, server_shutdown, server) = draft_server().await;
    let worker = Worker {
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
    ServiceFixture {
        _directory: directory,
        store,
        worker,
        server_shutdown,
        server,
    }
}

async fn admit_work(store: &SurfaceStore, update_id: i64, kind: InboundKind) -> WorkItem {
    store
        .admit(ParsedUpdate {
            update_id,
            canonical: format!("telegram-update-{update_id}").into_bytes(),
            topic: Some(Topic {
                chat_id: 42,
                thread_id: None,
            }),
            message_id: Some(update_id + 9),
            kind,
        })
        .await
        .expect("admit Telegram work");
    let PendingAction::Execute(work) = store
        .next_action()
        .await
        .expect("load work")
        .expect("queued work")
    else {
        panic!("update did not become executable");
    };
    work
}

async fn ready_delivery(store: &SurfaceStore) -> DeliveryItem {
    let PendingAction::Deliver(delivery) = store
        .next_action()
        .await
        .expect("load result")
        .expect("ready result")
    else {
        panic!("result did not become deliverable");
    };
    delivery
}

async fn finish_delivery(store: &SurfaceStore, delivery: DeliveryItem, message_id: i64) {
    store
        .mark_delivering(delivery.update_id)
        .await
        .expect("begin delivery");
    store
        .mark_chunk_delivered(delivery.update_id, delivery.cursor, message_id, true)
        .await
        .expect("finish delivery");
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
const modelSpec = process.env.RENOA_MODEL_SPEC;
if (action === "catalog") {
  process.stdout.write(JSON.stringify({ ok: true, response: { models: [
  {
    id: "fixture-model",
    name: "Fixture Model",
    reasoning_levels: ["low", "high"],
    context_window_tokens: 1000000,
    model_spec: { id: "fixture-model" }
  },
  {
    id: "fixture-model-b",
    name: "Fixture Model B",
    reasoning_levels: ["low", "high"],
    context_window_tokens: 1000000,
    model_spec: { id: "fixture-model-b" }
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
    metadata: {
      api: "test",
      provider: process.env.RENOA_MODEL_PROVIDER,
      model: JSON.parse(modelSpec).id
    }
  }
}) + "\n");
"#;
