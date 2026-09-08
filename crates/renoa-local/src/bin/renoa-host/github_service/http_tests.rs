use super::*;
use renoa_local::{LocalHostAdapters, LocalModelConfiguration, ModelProvider, arcee_profile};
use ring::hmac;

#[tokio::test]
async fn real_http_receiver_authenticates_raw_bytes_and_replays_durable_receipts() {
    let root = tempfile::tempdir().expect("Host");
    let host = LocalHost::new(
        root.path(),
        LocalModelConfiguration::new(
            root.path().join("bridge.mjs"),
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "fixture",
            root.path().join("auth.sqlite"),
        ),
        vec![arcee_profile(root.path()).expect("profile")],
        LocalHostAdapters::new(None),
    )
    .expect("Host");
    let secret = b"webhook test authentication secret".to_vec();
    let stop = CancellationToken::new();
    let state = Arc::new(State {
        host,
        secret: secret.clone(),
        stop: stop.clone(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let url = format!(
        "http://{}/v1/github/webhook",
        listener.local_addr().expect("address")
    );
    let server_stop = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(server_stop.cancelled_owned())
            .await
            .expect("server");
    });
    let id = Uuid::new_v4();
    let client = reqwest::Client::new();
    let send = |body: &'static str, signed: bool| {
        let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &secret), body.as_bytes());
        let mut signature = "sha256=".to_owned();
        for byte in tag.as_ref() {
            use std::fmt::Write as _;
            write!(signature, "{byte:02x}").expect("hex");
        }
        client
            .post(&url)
            .header("x-github-delivery", id.to_string())
            .header("x-github-event", "ping")
            .header(
                "x-hub-signature-256",
                if signed {
                    signature
                } else {
                    "sha256=bad".to_owned()
                },
            )
            .body(body)
            .send()
    };
    assert_eq!(
        send("{}", false).await.expect("unsigned").status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send("{}", true).await.expect("admission").status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send("{}", true).await.expect("replay").status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send("{\"different\":true}", true)
            .await
            .expect("conflict")
            .status(),
        StatusCode::CONFLICT
    );
    stop.cancel();
    server.await.expect("joined");
    let db = rusqlite::Connection::open(root.path().join("host.sqlite3")).expect("database");
    assert_eq!(
        db.query_row("SELECT count(*) FROM host_review_deliveries", [], |row| row
            .get::<_, i64>(0))
            .expect("durable receipt"),
        1
    );
}
