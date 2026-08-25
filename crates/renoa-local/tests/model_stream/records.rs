use renoa_agent::AssistantDelta;

use super::*;

#[tokio::test]
async fn model_forwards_content_deltas_before_the_completed_response() {
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
async fn missing_inference_outcome_is_unknown_not_invented_known_not_started() {
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
  error: "adapter omitted inference_outcome",
  error_kind: "invalid_request"
}}) + "\n");
"#
        ),
    )
    .expect("write malformed error bridge");
    let model = load_model(&bridge, &auth_store).await;

    let events = model
        .stream(request(), CancellationToken::new())
        .collect::<Vec<_>>()
        .await;

    assert_eq!(events.len(), 1);
    let Err(error) = &events[0] else {
        panic!("missing inference_outcome must surface as a model error");
    };
    assert_eq!(error.kind(), renoa_agent::ModelErrorKind::InvalidRequest);
    assert_eq!(
        error.inference_outcome(),
        renoa_agent::InferenceOutcome::Unknown
    );
}
