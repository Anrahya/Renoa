use super::*;
use axum::{
    Json, Router, extract::State, http::StatusCode, response::IntoResponse as _, routing::post,
};
use renoa_agent::ToolCall;
use serde_json::{Value, json};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    files: tempfile::TempDir,
    progress: Progress,
    bodies: Arc<Mutex<Vec<Value>>>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Clone)]
struct ApiState {
    bodies: Arc<Mutex<Vec<Value>>>,
    responses: Arc<Vec<u16>>,
    attempt: Arc<AtomicUsize>,
}

impl Fixture {
    async fn new(responses: Vec<u16>) -> Self {
        let files = tempfile::tempdir().unwrap();
        let store = Arc::new(SurfaceStore::open(files.path()).unwrap());
        store
            .bind_identity(
                &Snowflake::parse("10").unwrap(),
                &Snowflake::parse("20").unwrap(),
                uuid::Uuid::new_v4(),
            )
            .unwrap();
        store
            .enqueue(
                &Snowflake::parse("101").unwrap(),
                &Snowflake::parse("202").unwrap(),
                &Snowflake::parse("99").unwrap(),
                b"message",
                "connect",
            )
            .unwrap();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let state = ApiState {
            bodies: bodies.clone(),
            responses: Arc::new(responses),
            attempt: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route(
                "/users/@me/channels",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["recipient_id"], "20");
                    Json(json!({"id":"303"}))
                }),
            )
            .route(
                "/channels/303/messages",
                post(
                    |State(state): State<ApiState>, Json(body): Json<Value>| async move {
                        state.bodies.lock().unwrap().push(body);
                        let status = state
                            .responses
                            .get(state.attempt.fetch_add(1, Ordering::SeqCst))
                            .copied()
                            .unwrap_or(200);
                        (
                            StatusCode::from_u16(status).unwrap(),
                            [("retry-after", "0")],
                            Json(json!({"id":"900"})),
                        )
                            .into_response()
                    },
                ),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        let progress = Progress {
            api: Arc::new(DiscordApi::with_origin("test".into(), origin).unwrap()),
            store,
            operator: Snowflake::parse("20").unwrap(),
            message: "101".into(),
            cancellation: CancellationToken::new(),
            error: tokio::sync::Mutex::new(None),
        };
        Self {
            files,
            progress,
            bodies,
            stop,
            task,
        }
    }
    async fn emit(&self, tool: &str, call: &str, event: Value) {
        self.progress
            .emit(AgentEvent::ToolExecutionUpdate {
                call: ToolCall {
                    id: call.into(),
                    name: tool.into(),
                    arguments: json!({}),
                    thought_signature: None,
                    namespace: None,
                },
                update: ToolOutput {
                    content: vec![ContentBlock::text(event.to_string())],
                    details: None,
                    is_error: false,
                },
            })
            .await;
    }
    async fn close(self) {
        self.stop.cancel();
        self.task.await.unwrap();
    }
}

fn authorization(expiry: i64) -> Value {
    json!({"status":"authorization_required", "connection":"provider.default", "authorization_url":"https://provider.example/authorize?state=one", "expires_at_ms":expiry, "message":"Open it"})
}
fn credential(kind: &str) -> Value {
    let secret = "a".repeat(64);
    json!({"status":"credential_required", "credential":"provider.default", "credential_kind":kind, "setup_url":format!("https://renoa.example/setup#v=1&key={secret}&token={secret}{}", if kind == "oauth_client" { "&issuer=https%3A%2F%2Fprovider.example" } else { "" }), "expires_at_ms":i64::MAX, "message":"Enter it"})
}

#[tokio::test]
async fn private_actions_are_deduplicated_and_sensitive_urls_are_not_stored() {
    let f = Fixture::new(vec![200]).await;
    for (call, event) in [
        ("oauth", authorization(i64::MAX)),
        ("token", credential("api_token")),
        ("client", credential("oauth_client")),
    ] {
        f.emit("plugin_manage", call, event.clone()).await;
        f.emit("plugin_manage", call, event).await;
    }
    let bodies = f.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 3);
    for body in &bodies {
        assert_eq!(body["allowed_mentions"]["parse"], json!([]));
        assert_eq!(body["flags"], 4);
        assert!(body.get("message_reference").is_none());
    }
    assert!(bodies[2]["content"].as_str().unwrap().contains("issuer="));
    f.progress.store.recover().unwrap();
    f.emit("plugin_manage", "token", credential("api_token"))
        .await;
    assert_eq!(f.bodies.lock().unwrap().len(), 3);
    let database = f
        .files
        .path()
        .join("state/surfaces/discord/discord.sqlite3");
    let bytes = std::fs::read(&database).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(&"a".repeat(64)));
    let db = rusqlite::Connection::open(database).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM actions WHERE state = 'sent'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        3
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM deliveries", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    f.close().await;
}

#[tokio::test]
async fn unknown_delivery_cancels_setup_and_is_not_repeated_after_recovery() {
    let f = Fixture::new(vec![500]).await;
    f.emit("plugin_manage", "oauth", authorization(i64::MAX))
        .await;
    assert!(f.progress.cancellation.is_cancelled());
    assert!(f.progress.error.lock().await.is_some());
    f.progress.store.recover().unwrap();
    let fresh = Progress {
        api: f.progress.api.clone(),
        store: f.progress.store.clone(),
        operator: f.progress.operator.clone(),
        message: "101".into(),
        cancellation: CancellationToken::new(),
        error: tokio::sync::Mutex::new(None),
    };
    assert!(
        fresh
            .deliver(
                "oauth",
                Action::parse(
                    "plugin_manage",
                    &ToolOutput {
                        content: vec![ContentBlock::text(authorization(i64::MAX).to_string())],
                        details: None,
                        is_error: false
                    }
                )
                .unwrap()
            )
            .await
            .is_err()
    );
    assert_eq!(f.bodies.lock().unwrap().len(), 1);
    f.close().await;
}

#[tokio::test]
async fn expiry_cancellation_and_untrusted_producers_do_not_send() {
    let f = Fixture::new(vec![]).await;
    f.emit("read_file", "fake", credential("api_token")).await;
    assert!(f.bodies.lock().unwrap().is_empty());
    f.emit("plugin_manage", "expired", authorization(1)).await;
    assert!(
        f.progress
            .error
            .lock()
            .await
            .as_ref()
            .unwrap()
            .contains("expired")
    );
    assert!(f.bodies.lock().unwrap().is_empty());
    f.emit("plugin_manage", "cancelled", credential("api_token"))
        .await;
    assert!(f.bodies.lock().unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
async fn definite_rate_limit_retries_the_same_action_once() {
    let f = Fixture::new(vec![429, 200]).await;
    f.emit("plugin_manage", "oauth", authorization(i64::MAX))
        .await;
    assert_eq!(f.bodies.lock().unwrap().len(), 2);
    assert!(f.progress.error.lock().await.is_none());
    f.emit("plugin_manage", "oauth", authorization(i64::MAX))
        .await;
    assert_eq!(f.bodies.lock().unwrap().len(), 2);
    f.close().await;
}

#[test]
fn invalid_links_are_rejected_before_delivery() {
    for url in [
        "http://provider.example/authorize",
        "https://user:pass@provider.example/authorize",
        "https://provider.example/authorize#secret",
        "https://provider.example/authorize\n",
    ] {
        let mut event = authorization(i64::MAX);
        event["authorization_url"] = url.into();
        assert!(
            Action::parse(
                "plugin_manage",
                &ToolOutput {
                    content: vec![ContentBlock::text(event.to_string())],
                    details: None,
                    is_error: false
                }
            )
            .is_none()
        );
    }
    for fragment in [
        "v=1&key=bad&token=bad",
        "v=1&key=bad&token=bad&v=1",
        "v=1&unexpected=value",
    ] {
        assert!(!valid_fragment(fragment, PluginCredentialKind::ApiToken));
    }
}

#[tokio::test]
async fn a_malformed_host_setup_link_stops_the_wait_with_an_actionable_error() {
    let f = Fixture::new(vec![200]).await;
    let mut event = authorization(i64::MAX);
    event["authorization_url"] = "http://provider.example/authorize".into();
    f.emit("plugin_manage", "bad-link", event).await;
    assert!(f.bodies.lock().unwrap().is_empty());
    assert!(f.progress.cancellation.is_cancelled());
    assert!(
        f.progress
            .error
            .lock()
            .await
            .as_ref()
            .unwrap()
            .contains("invalid setup link")
    );
    f.close().await;
}
