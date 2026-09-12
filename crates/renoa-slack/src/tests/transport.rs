use super::*;
use crate::api::ApiError;

#[tokio::test]
async fn posts_and_streaming_updates_use_native_markdown_with_a_text_fallback() {
    let fixture = Fixture::new().await;
    let text = "**What it is**\n- First item\n- Second item\n\n[Docs](https://example.com)\n\n```rust\nlet x = 1 < 2;\n```";
    let topic = crate::ingress::Topic {
        channel: "C1".to_owned(),
        thread: "1.000001".to_owned(),
    };
    let message = fixture.worker.api.post(&topic, text).await.expect("post");
    fixture
        .worker
        .api
        .update(&topic, &message.ts, "**Next**\n\n`done`")
        .await
        .expect("update");
    let sent = fixture.sent.lock().await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["thread_ts"], topic.thread);
    assert_eq!(sent[1]["ts"], message.ts);
    for (body, expected) in sent.iter().zip([text, "**Next**\n\n`done`"]) {
        assert_eq!(body["blocks"], json!([{"type":"markdown","text":expected}]));
        assert_eq!(body["text"], expected);
        assert_eq!(body["parse"], "none");
        assert_eq!(body["unfurl_links"], false);
        assert_eq!(body["unfurl_media"], false);
    }
    drop(sent);
    fixture.stop().await;
}

#[tokio::test]
async fn slack_rate_limit_and_ambiguous_send_are_distinct_delivery_outcomes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let origin = url::Url::parse(&format!(
        "http://{}/",
        listener.local_addr().expect("address")
    ))
    .expect("origin");
    let app = Router::new()
        .route(
            "/chat.postMessage",
            post(|| async {
                (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "91")],
                    "slow down",
                )
            }),
        )
        .route(
            "/chat.update",
            post(|| async {
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "accepted but response lost",
                )
            }),
        );
    let stop = CancellationToken::new();
    let shutdown = stop.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
    });
    let api =
        SlackApi::with_origin("xoxb-test".to_owned(), "xapp-test".to_owned(), origin).expect("api");
    let topic = crate::ingress::Topic {
        channel: "D1".to_owned(),
        thread: String::new(),
    };
    assert!(
        matches!(api.post(&topic,"hello").await,Err(ApiError::RateLimited(delay)) if delay.as_secs()==91)
    );
    assert!(matches!(
        api.update(&topic, "1.000001", "hello").await,
        Err(ApiError::Unknown(_))
    ));
    stop.cancel();
    task.await.expect("server task").expect("server exit");
}
