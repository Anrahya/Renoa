use std::{fs, time::Duration};

use renoa_agent::{
    AssistantContent, ContentBlock, ModelRequest, StopReason, TokenUsage, sample_model,
};
use renoa_local::PiModel;
use tempfile::tempdir;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn pi_model_crosses_the_process_boundary_with_one_exact_request() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        r#"
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
if (request.system_prompt !== "Be precise." || request.messages[0].content[0].text !== "Hello") {
  process.stdout.write(JSON.stringify({ ok: false, error: "request changed" }));
} else if (process.env.RENOA_PI_PROVIDER !== "xai" || process.env.RENOA_PI_MODEL !== "grok-test") {
  process.stdout.write(JSON.stringify({ ok: false, error: "model configuration missing" }));
} else {
  process.stdout.write(JSON.stringify({
    ok: true,
    response: {
      content: [{ type: "text", text: "Hello back." }],
      stop_reason: "stop",
      usage: { input: 4, output: 2, cache_read: 1, cache_write: 0 },
      metadata: { api: "test", provider: "xai", model: "grok-test", response_id: "response-1" }
    }
  }));
}
"#,
    )
    .expect("write model bridge");
    let model =
        PiModel::new(&bridge, "xai", "grok-test", &auth_store).expect("configure Pi model adapter");

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
    .await
    .expect("sample through Pi adapter");

    assert_eq!(
        sampled.response.content,
        vec![AssistantContent::text("Hello back.")]
    );
    assert_eq!(sampled.response.stop_reason, StopReason::Stop);
    assert_eq!(
        sampled.response.usage,
        Some(TokenUsage {
            input: 4,
            output: 2,
            cache_read: 1,
            cache_write: 0,
        })
    );
    assert_eq!(
        sampled.response.metadata.response_id.as_deref(),
        Some("response-1")
    );
}

#[tokio::test]
async fn cancelling_a_pi_model_request_stops_its_bridge_process() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    let started = directory.path().join("started");
    let completed = directory.path().join("completed");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"
import {{ writeFileSync }} from "node:fs";
let input = "";
for await (const chunk of process.stdin) input += chunk;
JSON.parse(input);
writeFileSync({}, "started");
await new Promise(resolve => setTimeout(resolve, 800));
writeFileSync({}, "completed");
process.stdout.write(JSON.stringify({{
  ok: true,
  response: {{
    content: [{{ type: "text", text: "too late" }}],
    stop_reason: "stop",
    usage: null,
    metadata: {{ api: "test", provider: "xai", model: "grok-test", response_id: null }}
  }}
}}));
"#,
            serde_json::to_string(&started).expect("encode started path"),
            serde_json::to_string(&completed).expect("encode completed path"),
        ),
    )
    .expect("write model bridge");
    let model =
        PiModel::new(&bridge, "xai", "grok-test", &auth_store).expect("configure Pi model adapter");
    let cancellation = CancellationToken::new();
    let sampling_cancellation = cancellation.clone();
    let sampling = tokio::spawn(async move {
        sample_model(
            &model,
            ModelRequest {
                system_prompt: "Be precise.".to_owned(),
                messages: vec![renoa_agent::Message::User {
                    content: vec![ContentBlock::text("Hello")],
                }],
                tools: Vec::new(),
            },
            sampling_cancellation,
            None,
        )
        .await
    });

    timeout(Duration::from_secs(2), async {
        while !started.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bridge starts");
    cancellation.cancel();

    let Err(error) = timeout(Duration::from_secs(2), sampling)
        .await
        .expect("cancellation settles")
        .expect("sampling task completes")
    else {
        panic!("cancelled sampling succeeded");
    };
    assert_eq!(error.to_string(), "model sampling was cancelled");
    sleep(Duration::from_secs(1)).await;
    assert!(!completed.exists(), "cancelled bridge kept running");
}
