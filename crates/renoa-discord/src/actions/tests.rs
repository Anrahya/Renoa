use super::*;
use axum::{
    Json, Router, extract::State, http::StatusCode, response::IntoResponse as _, routing::post,
};
use renoa_agent::{ContentBlock, ToolOutput};
use serde_json::{Value, json};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

const COMMAND: &str = "33333333-3333-4333-8333-333333333333";

struct Fixture {
    files: tempfile::TempDir,
    progress: SetupDelivery,
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
        let progress = SetupDelivery {
            api: Arc::new(DiscordApi::with_origin("test".into(), origin).unwrap()),
            store,
            operator: Snowflake::parse("20").unwrap(),
            command: COMMAND.into(),
            cancellation: CancellationToken::new(),
        };
        Self {
            files,
            progress,
            bodies,
            stop,
            task,
        }
    }
    async fn deliver(&self, call: &str, event: Value) -> Result<(), DiscordError> {
        self.progress.deliver(call, &link(&event)).await
    }
    async fn close(self) {
        self.stop.cancel();
        self.task.await.unwrap();
    }
}

/// The link as the node passes it on, after the shared check.
fn link(event: &Value) -> SetupLink {
    let update = ToolOutput {
        content: vec![ContentBlock::text(event.to_string())],
        details: None,
        is_error: false,
        closing: None,
    };
    SetupLink::from_update("plugin_manage", &update)
        .expect("setup progress")
        .expect("a valid link")
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
        f.deliver(call, event.clone()).await.unwrap();
        f.deliver(call, event).await.unwrap();
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
    f.deliver("token", credential("api_token")).await.unwrap();
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
async fn an_unknown_delivery_fails_and_is_not_repeated_after_recovery() {
    let f = Fixture::new(vec![500]).await;
    assert!(f.deliver("oauth", authorization(i64::MAX)).await.is_err());
    f.progress.store.recover().unwrap();
    let fresh = SetupDelivery {
        api: f.progress.api.clone(),
        store: f.progress.store.clone(),
        operator: f.progress.operator.clone(),
        command: COMMAND.into(),
        cancellation: CancellationToken::new(),
    };
    assert!(
        fresh
            .deliver("oauth", &link(&authorization(i64::MAX)))
            .await
            .is_err()
    );
    assert_eq!(f.bodies.lock().unwrap().len(), 1);
    f.close().await;
}

#[tokio::test]
async fn an_expired_link_fails_and_a_cancelled_command_sends_nothing() {
    let f = Fixture::new(vec![]).await;
    let expired = f.deliver("expired", authorization(1)).await;
    assert!(expired.unwrap_err().to_string().contains("expired"));
    assert!(f.bodies.lock().unwrap().is_empty());
    f.progress.cancellation.cancel();
    f.deliver("cancelled", credential("api_token"))
        .await
        .unwrap();
    assert!(f.bodies.lock().unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
async fn definite_rate_limit_retries_the_same_action_once() {
    let f = Fixture::new(vec![429, 200]).await;
    f.deliver("oauth", authorization(i64::MAX)).await.unwrap();
    assert_eq!(f.bodies.lock().unwrap().len(), 2);
    f.deliver("oauth", authorization(i64::MAX)).await.unwrap();
    assert_eq!(f.bodies.lock().unwrap().len(), 2);
    f.close().await;
}
