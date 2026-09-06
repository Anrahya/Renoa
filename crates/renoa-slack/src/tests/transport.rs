use super::*;
use crate::api::ApiError;

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
