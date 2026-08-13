use std::{fs, num::NonZeroU32, time::Duration};

use renoa_agent::{
    AssistantContent, ContentBlock, ModelErrorKind, ModelRequest, SamplingError, StopReason,
    TokenUsage, sample_model,
};
use renoa_local::{PiModel, PiModelConfigError};
use tempfile::tempdir;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

const TEST_MODEL_BINDING_ID: &str =
    "15fa2142926bbf4af032a8a733095d6127ca0a041e85ee583e25bc635821fd21";

#[tokio::test]
async fn pi_model_uses_a_smaller_provider_output_limit() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        r#"
process.stdout.write(JSON.stringify({
  ok: true,
  response: {
    context_window_tokens: 100000,
    max_output_tokens: 8192,
    model_spec: "{}",
    model_binding_id: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
  }
}));
"#,
    )
    .expect("write model bridge");

    let model = PiModel::load(
        &bridge,
        "xai",
        "grok-test",
        &auth_store,
        NonZeroU32::new(32_768).expect("non-zero host cap"),
    )
    .await
    .expect("provider limit is a valid lower cap");

    assert_eq!(model.max_output_tokens().get(), 8_192);
}

#[tokio::test]
async fn pi_model_rejects_a_model_spec_with_the_wrong_binding_id() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        r#"
process.stdout.write(JSON.stringify({
  ok: true,
  response: {
    context_window_tokens: 100000,
    max_output_tokens: 8192,
    model_spec: "{}",
    model_binding_id: "0000000000000000000000000000000000000000000000000000000000000000"
  }
}));
"#,
    )
    .expect("write model bridge");

    let result = PiModel::load(
        &bridge,
        "xai",
        "grok-test",
        &auth_store,
        NonZeroU32::new(32_768).expect("non-zero host cap"),
    )
    .await;

    assert!(matches!(
        result,
        Err(PiModelConfigError::InvalidModelBindingId)
    ));
}

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
if (process.env.RENOA_PI_ACTION === "describe") {
  process.stdout.write(JSON.stringify({
    ok: true,
    response: {
      context_window_tokens: 500000,
      max_output_tokens: 500000,
      model_spec: "{\"id\":\"grok-test\"}",
      model_binding_id: "15fa2142926bbf4af032a8a733095d6127ca0a041e85ee583e25bc635821fd21"
    }
  }));
} else if (process.env.RENOA_PI_ACTION !== "invoke") {
  process.stdout.write(JSON.stringify({ ok: false, error: "unknown action" }));
} else if (!process.execArgv.includes("--dns-result-order=ipv4first")) {
  process.stdout.write(JSON.stringify({ ok: false, error: "DNS address order missing" }));
} else if (process.env.RENOA_PI_MODEL_SPEC !== "{\"id\":\"grok-test\"}") {
  process.stdout.write(JSON.stringify({ ok: false, error: "model binding missing" }));
} else if (process.env.RENOA_PI_MAX_OUTPUT_TOKENS !== "32768") {
  process.stdout.write(JSON.stringify({ ok: false, error: "output cap missing" }));
} else if (JSON.parse(input).system_prompt !== "Be precise." || JSON.parse(input).messages[0].content[0].text !== "Hello") {
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
    let model = PiModel::load(
        &bridge,
        "xai",
        "grok-test",
        &auth_store,
        NonZeroU32::new(32_768).expect("non-zero output cap"),
    )
    .await
    .expect("configure Pi model adapter");
    assert_eq!(model.context_window_tokens().get(), 500_000);
    assert_eq!(model.max_output_tokens().get(), 32_768);
    assert_eq!(model.binding_id(), TEST_MODEL_BINDING_ID);

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
if (process.env.RENOA_PI_ACTION === "describe") {{
  process.stdout.write(JSON.stringify({{
    ok: true,
    response: {{
      context_window_tokens: 500000,
      max_output_tokens: 500000,
      model_spec: "{{}}",
      model_binding_id: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    }}
  }}));
  process.exit(0);
}}
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
    let model = PiModel::load(
        &bridge,
        "xai",
        "grok-test",
        &auth_store,
        NonZeroU32::new(32_768).expect("non-zero output cap"),
    )
    .await
    .expect("configure Pi model adapter");
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

#[tokio::test]
async fn pi_model_preserves_a_known_context_rejection() {
    let directory = tempdir().expect("temporary directory");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        r#"
let input = "";
for await (const chunk of process.stdin) input += chunk;
if (process.env.RENOA_PI_ACTION === "describe") {
  process.stdout.write(JSON.stringify({
    ok: true,
    response: {
      context_window_tokens: 500000,
      max_output_tokens: 500000,
      model_spec: "{}",
      model_binding_id: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    }
  }));
} else {
  JSON.parse(input);
  process.stdout.write(JSON.stringify({
    ok: false,
    error: "maximum prompt length is 500000 but request contains 500001 tokens",
    error_kind: "context_window_exceeded"
  }));
}
"#,
    )
    .expect("write model bridge");
    let model = PiModel::load(
        &bridge,
        "xai",
        "grok-test",
        &auth_store,
        NonZeroU32::new(32_768).expect("non-zero output cap"),
    )
    .await
    .expect("configure Pi model adapter");

    let result = sample_model(
        &model,
        ModelRequest {
            system_prompt: "Be precise.".to_owned(),
            messages: Vec::new(),
            tools: Vec::new(),
        },
        CancellationToken::new(),
        None,
    )
    .await;
    assert!(matches!(
        result,
        Err(SamplingError::Model(error))
            if error.kind() == ModelErrorKind::ContextWindowExceeded
    ));
}
