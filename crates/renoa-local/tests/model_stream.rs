use std::{fs, num::NonZeroU32, time::Duration};

use futures_util::StreamExt as _;
use renoa_agent::{AssistantContent, ContentBlock, Model, ModelEvent, ModelRequest};
use renoa_local::BridgeModel;
use tempfile::tempdir;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

#[path = "model_stream/records.rs"]
mod records;

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

#[tokio::test]
async fn cancellation_after_the_bridge_emits_completed_keeps_the_response() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    let ready = directory.path().join("ready");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"{}
import {{ writeFileSync }} from "node:fs";
for await (const _chunk of process.stdin) {{}}
process.stdout.write(JSON.stringify({{
  event: "completed",
  response: {{
    content: [{{ type: "text", text: "kept" }}],
    stop_reason: "stop",
    usage: {{ input: 1, output: 1, cache_read: 0, cache_write: 0 }},
    metadata: {{ api: "test", provider: "xai", model: "grok-test" }}
  }}
}}) + "\n");
writeFileSync({}, "ready");
await new Promise(() => {{}});
"#,
            DESCRIPTION,
            serde_json::to_string(&ready).expect("encode ready path"),
        ),
    )
    .expect("write completed-then-hang bridge");
    let model = load_model(&bridge, &auth_store).await;
    let cancellation = CancellationToken::new();
    let stream = model.stream(request(), cancellation.clone());
    timeout(Duration::from_secs(2), async {
        while !ready.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bridge emits completed");
    cancellation.cancel();
    let events = timeout(Duration::from_secs(2), stream.collect::<Vec<_>>())
        .await
        .expect("cancelled stream must settle");
    assert!(
        events.iter().any(|event| matches!(
            event,
            Ok(ModelEvent::Completed { response })
                if response.content == vec![AssistantContent::text("kept")]
        )),
        "completed response must survive cancellation: {events:?}"
    );
    assert!(
        !events.iter().any(|event| matches!(event, Err(error) if error.kind() == renoa_agent::ModelErrorKind::Cancelled)),
        "cancellation must not replace a parsed completed record: {events:?}"
    );
}

#[tokio::test]
async fn a_structured_stream_error_survives_a_nonzero_adapter_exit() {
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
  event: "error",
  error: "xAI request failed after 1 attempt: invalid request.",
  error_kind: "invalid_request",
  inference_outcome: "known_not_started"
}}) + "\n");
process.exit(1);
"#
        ),
    )
    .expect("write error-then-exit bridge");
    let model = load_model(&bridge, &auth_store).await;
    let events = model
        .stream(request(), CancellationToken::new())
        .collect::<Vec<_>>()
        .await;
    assert_eq!(events.len(), 1, "{events:?}");
    let Err(error) = &events[0] else {
        panic!("structured error must surface as a model error: {events:?}");
    };
    assert_eq!(error.kind(), renoa_agent::ModelErrorKind::InvalidRequest);
    assert!(
        error.to_string().contains("invalid request"),
        "nonzero exit must not replace the structured error: {error}"
    );
    assert!(
        !error.to_string().contains("exited with"),
        "child exit status must not overwrite the stream error: {error}"
    );
}

#[tokio::test]
async fn a_completed_record_survives_broken_stdin_write() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"{DESCRIPTION}
process.stdout.write(JSON.stringify({{
  event: "completed",
  response: {{
    content: [{{ type: "text", text: "kept-after-stdin" }}],
    stop_reason: "stop",
    usage: {{ input: 1, output: 1, cache_read: 0, cache_write: 0 }},
    metadata: {{ api: "test", provider: "xai", model: "grok-test" }}
  }}
}}) + "\n");
process.exit(0);
"#
        ),
    )
    .expect("write completed-without-stdin bridge");
    let model = load_model(&bridge, &auth_store).await;
    let events = model
        .stream(request(), CancellationToken::new())
        .collect::<Vec<_>>()
        .await;
    assert!(
        events.iter().any(|event| matches!(
            event,
            Ok(ModelEvent::Completed { response })
                if response.content == vec![AssistantContent::text("kept-after-stdin")]
        )),
        "completed response must survive a broken stdin write: {events:?}"
    );
    assert!(
        !events.iter().any(Result::is_err),
        "stdin write failure must not replace a parsed completed record: {events:?}"
    );
}

#[tokio::test]
async fn a_completed_record_survives_oversized_stderr() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"{DESCRIPTION}
process.stdout.write(JSON.stringify({{
  event: "completed",
  response: {{
    content: [{{ type: "text", text: "kept-after-stderr" }}],
    stop_reason: "stop",
    usage: {{ input: 1, output: 1, cache_read: 0, cache_write: 0 }},
    metadata: {{ api: "test", provider: "xai", model: "grok-test" }}
  }}
}}) + "\n");
for (let i = 0; i < 20; i++) {{
  process.stderr.write("x".repeat(1024 * 1024));
}}
process.exit(0);
"#
        ),
    )
    .expect("write completed-with-stderr bridge");
    let model = load_model(&bridge, &auth_store).await;
    let events = timeout(
        Duration::from_secs(10),
        model
            .stream(request(), CancellationToken::new())
            .collect::<Vec<_>>(),
    )
    .await
    .expect("oversized stderr stream must settle");
    assert!(
        events.iter().any(|event| matches!(
            event,
            Ok(ModelEvent::Completed { response })
                if response.content == vec![AssistantContent::text("kept-after-stderr")]
        )),
        "completed response must survive oversized stderr: {events:?}"
    );
    assert!(
        !events.iter().any(Result::is_err),
        "oversized stderr must not replace a parsed completed record: {events:?}"
    );
}

#[tokio::test]
async fn a_completed_record_is_published_before_a_hung_adapter_exits() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"{DESCRIPTION}
process.stdout.write(JSON.stringify({{
  event: "completed",
  response: {{
    content: [{{ type: "text", text: "kept-while-hung" }}],
    stop_reason: "stop",
    usage: {{ input: 1, output: 1, cache_read: 0, cache_write: 0 }},
    metadata: {{ api: "test", provider: "xai", model: "grok-test" }}
  }}
}}) + "\n");
await new Promise(() => {{}});
"#
        ),
    )
    .expect("write hung-after-completed bridge");
    let model = load_model(&bridge, &auth_store).await;
    let events = timeout(
        Duration::from_secs(3),
        model
            .stream(request(), CancellationToken::new())
            .collect::<Vec<_>>(),
    )
    .await
    .expect("hung adapter must not delay a parsed completed record");
    assert!(
        events.iter().any(|event| matches!(
            event,
            Ok(ModelEvent::Completed { response })
                if response.content == vec![AssistantContent::text("kept-while-hung")]
        )),
        "completed response must be published before waiting on a hung child: {events:?}"
    );
    assert!(
        !events.iter().any(Result::is_err),
        "a hung adapter must not replace a parsed completed record: {events:?}"
    );
}

#[tokio::test]
async fn an_error_record_is_published_before_a_hung_adapter_exits() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            r#"{DESCRIPTION}
process.stdout.write(JSON.stringify({{
  event: "error",
  error: "kept-while-hung",
  error_kind: "invalid_request",
  inference_outcome: "known_not_started"
}}) + "\n");
await new Promise(() => {{}});
"#
        ),
    )
    .expect("write hung-after-error bridge");
    let model = load_model(&bridge, &auth_store).await;
    let events = timeout(
        Duration::from_secs(3),
        model
            .stream(request(), CancellationToken::new())
            .collect::<Vec<_>>(),
    )
    .await
    .expect("hung adapter must not delay a parsed error record");
    assert!(
        events.iter().any(|event| matches!(
            event,
            Err(error) if error.to_string().contains("kept-while-hung")
        )),
        "error record must be published before waiting on a hung child: {events:?}"
    );
}

async fn load_model(bridge: &std::path::Path, auth_store: &std::path::Path) -> BridgeModel {
    BridgeModel::load(
        bridge,
        "xai",
        "grok-test",
        auth_store,
        None,
        NonZeroU32::new(32_768).expect("non-zero output cap"),
    )
    .await
    .expect("configure model adapter")
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
if (process.env.RENOA_MODEL_ACTION === "describe") {
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
