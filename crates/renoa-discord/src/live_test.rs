use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalHost,
    LocalHostAdapters, LocalModelConfiguration, ModelProvider,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    api::DiscordApi,
    service::{self, Surface},
    snowflake::Snowflake,
};

#[tokio::test]
async fn a_mention_runs_the_provisioned_agent_and_posts_one_reply() {
    let root = tempfile::tempdir().expect("temp root");
    let data = root.path().join("data");
    let workspace = root.path().join("workspace");
    let bridge = root.path().join("model-bridge.mjs");
    let credentials = root.path().join("credentials.sqlite3");
    std::fs::create_dir(&workspace).expect("workspace");
    std::fs::write(&bridge, MODEL_BRIDGE).expect("bridge");
    std::fs::write(&credentials, "").expect("credentials");
    let host = LocalHost::new(
        &data,
        LocalModelConfiguration::new(
            &bridge,
            vec![ModelProvider::OpenCodeGo],
            ModelProvider::OpenCodeGo,
            "fixture-model",
            &credentials,
        ),
        LocalHostAdapters::default(),
    )
    .expect("host");
    let agent_id = host
        .create_agent(
            AgentCreator::System {
                component: "discord-test".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new("renoa.personal.arcee.v2").expect("preset"),
                "Arcee",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("provision")
        .id;

    let posted = Arc::new(Mutex::new(Vec::new()));
    let gateway = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gateway listener");
    let gateway_port = gateway.local_addr().expect("gateway port").port();
    let http = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("http listener");
    let http_addr = http.local_addr().expect("http port");
    let posted_http = Arc::clone(&posted);
    tokio::spawn(async move { serve_http(http, gateway_port, posted_http).await });
    tokio::spawn(async move { serve_gateway(gateway).await });

    let shutdown = CancellationToken::new();
    let task_shutdown = shutdown.clone();
    let task = tokio::spawn(async move {
        service::run(
            Surface {
                host,
                agent_id: Uuid::parse_str(&agent_id.to_string()).expect("agent uuid"),
                workspace,
                guild_id: Snowflake::parse("10").expect("guild"),
                operator_user_id: Snowflake::parse("20").expect("operator"),
                token: "discord-token".to_owned(),
                data_directory: data,
            },
            DiscordApi::with_origin("discord-token".to_owned(), format!("http://{http_addr}"))
                .expect("api"),
            task_shutdown,
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if !posted.lock().expect("posted").is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timed out waiting for the Discord reply");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let bodies = posted.lock().expect("posted").clone();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(
        bodies[0].contains("Arcee completed the real path."),
        "{}",
        bodies[0]
    );
    shutdown.cancel();
    task.await.expect("service task").expect("service");
}

async fn serve_http(
    listener: tokio::net::TcpListener,
    gateway_port: u16,
    posted: Arc<Mutex<Vec<String>>>,
) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buffer = vec![0_u8; 8192];
        let Ok(read) = stream.read(&mut buffer).await else {
            continue;
        };
        let request = String::from_utf8_lossy(&buffer[..read]);
        let body = if request.contains("POST /channels/") {
            posted.lock().expect("posted").push(request.to_string());
            r#"{"id":"900"}"#.to_owned()
        } else {
            format!(r#"{{"url":"ws://127.0.0.1:{gateway_port}"}}"#)
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    }
}

async fn serve_gateway(listener: tokio::net::TcpListener) {
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;

    let Ok((stream, _)) = listener.accept().await else {
        return;
    };
    let mut socket = tokio_tungstenite::accept_async(stream)
        .await
        .expect("accept gateway");
    socket
        .send(Message::Text(
            r#"{"op":10,"d":{"heartbeat_interval":60000}}"#.into(),
        ))
        .await
        .expect("hello");
    let _ = socket.next().await;
    socket
        .send(Message::Text(
            r#"{"op":0,"s":1,"t":"READY","d":{"session_id":"sess","resume_gateway_url":"ws://127.0.0.1","user":{"id":"50"}}}"#.into(),
        ))
        .await
        .expect("ready");
    let message = r#"{"op":0,"s":2,"t":"MESSAGE_CREATE","d":{"id":"101","channel_id":"202","guild_id":"10","content":"<@50> Do the real task.","author":{"id":"99"},"mentions":[{"id":"50"}]}}"#;
    socket
        .send(Message::Text(message.into()))
        .await
        .expect("message");
    tokio::time::sleep(Duration::from_millis(200)).await;
    socket
        .send(Message::Text(message.into()))
        .await
        .expect("duplicate message");
    let _ = socket.next().await;
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
if (!request.system_prompt.startsWith("You are Arcee, Renoa's personal operator.")) process.exit(3);
const user = request.messages.at(-1);
if (user?.role !== "user" || !JSON.stringify(user.content).includes("Do the real task.")) process.exit(6);
process.stdout.write(JSON.stringify({
  event: "completed",
  response: {
    content: [{ type: "text", text: "Arcee completed the real path." }],
    stop_reason: "stop",
    usage: { input: 8, output: 4, cache_read: 0, cache_write: 0 },
    metadata: { api: "test", provider: process.env.RENOA_MODEL_PROVIDER, model: JSON.parse(modelSpec).id }
  }
}) + "\n");
"#;
