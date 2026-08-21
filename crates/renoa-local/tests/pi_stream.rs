use std::{fs, num::NonZeroU32, time::Duration};

use futures_util::StreamExt as _;
use renoa_agent::{
    AssistantContent, AssistantDelta, ContentBlock, Model, ModelEvent, ModelRequest,
};
use renoa_local::PiModel;
use tempfile::tempdir;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn pi_model_forwards_content_deltas_before_the_completed_response() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"{DESCRIPTION}
for await (const _chunk of process.stdin) {{}}
process.stdout.write(JSON.stringify({{
  event: "content_delta",
  content_index: 0,
  delta: {{ type: "text", text: "Hello " }}
}}) + "\n");
process.stdout.write(JSON.stringify({{
  event: "content_delta",
  content_index: 0,
  delta: {{ type: "text", text: "world" }}
}}) + "\n");
process.stdout.write(JSON.stringify({{
  event: "completed",
  response: {{
    content: [{{ type: "text", text: "Hello world" }}],
    stop_reason: "stop",
    usage: {{ input: 1, output: 2, cache_read: 0, cache_write: 0 }},
    metadata: {{ api: "test", provider: "xai", model: "grok-test" }}
  }}
}}) + "\n");
"#
        ),
    )
    .expect("write streaming bridge");
    let model = load_model(&bridge, &auth_store).await;

    let events = model
        .stream(request(), CancellationToken::new())
        .collect::<Vec<_>>()
        .await;

    assert_eq!(events.len(), 3);
    assert!(matches!(
        &events[0],
        Ok(ModelEvent::ContentDelta {
            content_index: 0,
            delta: AssistantDelta::Text { text }
        }) if text == "Hello "
    ));
    assert!(matches!(
        &events[1],
        Ok(ModelEvent::ContentDelta {
            content_index: 0,
            delta: AssistantDelta::Text { text }
        }) if text == "world"
    ));
    assert!(matches!(
        &events[2],
        Ok(ModelEvent::Completed { response })
            if response.content == vec![AssistantContent::text("Hello world")]
    ));
}

#[tokio::test]
async fn dropping_the_model_stream_stops_its_bridge_process() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    let started = directory.path().join("started");
    let completed = directory.path().join("completed");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"{}
import {{ writeFileSync }} from "node:fs";
for await (const _chunk of process.stdin) {{}}
writeFileSync({}, "started");
await new Promise(resolve => setTimeout(resolve, 800));
writeFileSync({}, "completed");
"#,
            DESCRIPTION,
            serde_json::to_string(&started).expect("encode started path"),
            serde_json::to_string(&completed).expect("encode completed path"),
        ),
    )
    .expect("write blocking bridge");
    let model = load_model(&bridge, &auth_store).await;
    let stream = model.stream(request(), CancellationToken::new());

    timeout(Duration::from_secs(2), async {
        while !started.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bridge starts");
    drop(stream);

    sleep(Duration::from_secs(1)).await;
    assert!(
        !completed.exists(),
        "dropped stream left its bridge running"
    );
}

#[tokio::test]
async fn cancellation_stops_a_bridge_while_stream_delivery_is_backpressured() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    let started = directory.path().join("started");
    let completed = directory.path().join("completed");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"{}
import {{ writeFileSync }} from "node:fs";
for await (const _chunk of process.stdin) {{}}
writeFileSync({}, "started");
for (const text of ["one", "two"]) {{
  process.stdout.write(JSON.stringify({{
    event: "content_delta",
    content_index: 0,
    delta: {{ type: "text", text }}
  }}) + "\n");
}}
await new Promise(resolve => setTimeout(resolve, 800));
writeFileSync({}, "completed");
"#,
            DESCRIPTION,
            serde_json::to_string(&started).expect("encode started path"),
            serde_json::to_string(&completed).expect("encode completed path"),
        ),
    )
    .expect("write backpressured bridge");
    let model = load_model(&bridge, &auth_store).await;
    let cancellation = CancellationToken::new();
    let stream = model.stream(request(), cancellation.clone());

    timeout(Duration::from_secs(2), async {
        while !started.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bridge starts");
    cancellation.cancel();

    sleep(Duration::from_secs(1)).await;
    assert!(
        !completed.exists(),
        "backpressure prevented cancellation from reaching the bridge"
    );
    drop(stream);
}

#[tokio::test]
async fn dropping_the_model_stream_stops_bridge_descendants() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    let started = directory.path().join("started");
    let descendant_completed = directory.path().join("descendant-completed");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"{}
import {{ spawn }} from "node:child_process";
import {{ writeFileSync }} from "node:fs";
for await (const _chunk of process.stdin) {{}}
writeFileSync({}, "started");
spawn(process.execPath, ["-e", {}], {{ stdio: "ignore" }});
await new Promise(resolve => setTimeout(resolve, 5000));
"#,
            DESCRIPTION,
            serde_json::to_string(&started).expect("encode started path"),
            serde_json::to_string(&format!(
                "setTimeout(() => require('node:fs').writeFileSync({}, 'completed'), 800)",
                serde_json::to_string(&descendant_completed).expect("encode completed path")
            ))
            .expect("encode descendant program"),
        ),
    )
    .expect("write descendant bridge");
    let model = load_model(&bridge, &auth_store).await;
    let stream = model.stream(request(), CancellationToken::new());

    timeout(Duration::from_secs(2), async {
        while !started.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bridge starts");
    drop(stream);

    sleep(Duration::from_secs(1)).await;
    assert!(
        !descendant_completed.exists(),
        "dropped stream left a bridge descendant running"
    );
}

async fn load_model(bridge: &std::path::Path, auth_store: &std::path::Path) -> PiModel {
    PiModel::load(
        bridge,
        "xai",
        "grok-test",
        auth_store,
        None,
        NonZeroU32::new(32_768).expect("non-zero output cap"),
    )
    .await
    .expect("configure Pi model adapter")
}

fn request() -> ModelRequest {
    ModelRequest {
        system_prompt: "Be precise.".to_owned(),
        messages: vec![renoa_agent::Message::User {
            content: vec![ContentBlock::text("Hello")],
        }],
        tools: Vec::new(),
    }
}

const DESCRIPTION: &str = r#"
if (process.env.RENOA_PI_ACTION === "describe") {
  process.stdout.write(JSON.stringify({
    ok: true,
    response: {
      context_window_tokens: 500000,
      max_output_tokens: 500000,
      model_spec: "{}",
      model_binding_id: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
      reasoning_level: "high"
    }
  }));
  process.exit(0);
}
"#;
