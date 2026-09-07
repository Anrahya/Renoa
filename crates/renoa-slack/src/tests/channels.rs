use super::*;
use crate::channels::Channels;
use axum::{extract::Request, http::StatusCode, response::IntoResponse as _, routing::any};
use std::collections::VecDeque;

#[derive(Default)]
struct Remote {
    requests: Mutex<Vec<String>>,
    responses: Mutex<VecDeque<(StatusCode, Value)>>,
    channel: Mutex<Value>,
}
struct TestApi {
    api: Arc<SlackApi>,
    remote: Arc<Remote>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl TestApi {
    async fn new() -> Self {
        let remote = Arc::new(Remote::default());
        let app = Router::new()
            .fallback(any(call))
            .with_state(Arc::clone(&remote));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let origin = url::Url::parse(&format!(
            "http://{}/",
            listener.local_addr().expect("address")
        ))
        .expect("origin");
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
        Self {
            api: Arc::new(
                SlackApi::with_origin("xoxb-test".to_owned(), "xapp-test".to_owned(), origin)
                    .expect("api"),
            ),
            remote,
            stop,
            task,
        }
    }
    fn worker(&self, fixture: &Fixture) -> Channels {
        Channels {
            host: fixture.worker.host.clone(),
            store: fixture.worker.store.clone(),
            api: Arc::clone(&self.api),
            bot: "U2".to_owned(),
            user: "U3".to_owned(),
            shutdown: CancellationToken::new(),
            wake: Arc::new(Notify::new()),
        }
    }
    async fn stop(self) {
        self.stop.cancel();
        self.task.await.expect("join").expect("server");
    }
}
async fn call(State(remote): State<Arc<Remote>>, request: Request) -> axum::response::Response {
    let path = request.uri().path().to_owned();
    remote.requests.lock().await.push(request.uri().to_string());
    let bytes = axum::body::to_bytes(request.into_body(), 4096)
        .await
        .expect("body");
    if path == "/conversations.create" {
        let body: Value = serde_json::from_slice(&bytes).expect("request");
        assert_eq!(body["is_private"], true);
        *remote.channel.lock().await = json!({"id":"CNEWS","name":body["name"],"creator":"U2","is_private":true,"is_archived":false});
    }
    if path == "/conversations.rename"
        && !remote
            .responses
            .lock()
            .await
            .front()
            .is_some_and(|(_, body)| body["error"] == "name_taken")
    {
        let body: Value = serde_json::from_slice(&bytes).expect("rename request");
        assert_eq!(body["channel"], "CNEWS");
        remote.channel.lock().await["name"] = body["name"].clone();
    }
    if let Some((status, body)) = remote.responses.lock().await.pop_front() {
        return (status, Json(body)).into_response();
    }
    let channel = remote.channel.lock().await.clone();
    let result = match path.as_str() {
        "/conversations.create" | "/conversations.rename" | "/conversations.info" => {
            json!({"ok":true,"channel":channel})
        }
        "/conversations.invite" => {
            let body: Value = serde_json::from_slice(&bytes).expect("invite");
            assert_eq!(body["channel"], "CNEWS");
            assert_eq!(body["users"], "U3");
            json!({"ok":true,"channel":channel})
        }
        "/conversations.list" => {
            json!({"ok":true,"channels":[channel],"response_metadata":{"next_cursor":""}})
        }
        _ => panic!("unexpected endpoint"),
    };
    Json(result).into_response()
}

#[tokio::test]
async fn private_channel_recovery_binds_plain_messages_to_one_durable_specialist_conversation() {
    let mut fixture = Fixture::new().await;
    let bot = super::agents::news_bot(&fixture).await;
    let remote = TestApi::new().await;
    let ignored = || {
        let mut input = envelope("E0", "0.000001", "before binding");
        input.payload.as_mut().expect("payload")["event"]["channel"] = json!("CNEWS");
        input.payload.as_mut().expect("payload")["event"]["channel_type"] = json!("group");
        input
    };
    fixture
        .receiver
        .admit_envelope(ignored())
        .await
        .expect("ignored receipt");
    remote
        .worker(&fixture)
        .provision(&summary(&bot))
        .await
        .expect("provision");
    fixture
        .receiver
        .admit_envelope(ignored())
        .await
        .expect("ignored retry stays ignored after channel binding");
    remote
        .worker(&fixture)
        .provision(&summary(&bot))
        .await
        .expect("idempotent restart");
    assert_eq!(
        *remote.remote.requests.lock().await,
        [
            "/conversations.create",
            "/conversations.invite",
            "/conversations.info?channel=CNEWS",
            "/conversations.rename"
        ]
    );
    for (event, ts, text) in [
        ("E1", "1.000001", "hello"),
        ("E2", "2.000001", "next"),
        ("E3", "3.000001", "!new"),
        ("E4", "4.000001", "after reset"),
    ] {
        let mut input = envelope(event, ts, text);
        input.payload.as_mut().expect("payload")["event"]["channel"] = json!("CNEWS");
        input.payload.as_mut().expect("payload")["event"]["channel_type"] = json!("group");
        fixture
            .receiver
            .admit_envelope(input)
            .await
            .expect("unmentioned message");
    }
    let mut sessions = Vec::new();
    for _ in 0..4 {
        let work = fixture
            .worker
            .store
            .next_work()
            .await
            .expect("queue")
            .expect("work");
        assert_eq!(work.topic.thread, "");
        assert_eq!(
            fixture
                .worker
                .store
                .session_agent(work.session_id)
                .await
                .expect("target")
                .to_string(),
            bot.id.to_string()
        );
        sessions.push(work.session_id);
        fixture
            .worker
            .execute(work)
            .await
            .expect("specialist execution");
    }
    assert_eq!(sessions[0], sessions[1]);
    assert_ne!(sessions[1], sessions[2]);
    assert_eq!(sessions[2], sessions[3]);
    assert!(
        fixture
            .worker
            .store
            .channel_description(bot.id.to_string())
            .await
            .expect("status")
            .ends_with("ready")
    );
    remote.stop().await;
    fixture.stop().await;
}

#[tokio::test]
async fn ambiguous_creation_is_looked_up_across_pages_without_another_create() {
    let fixture = Fixture::new().await;
    let bot = super::agents::news_bot(&fixture).await;
    let remote = TestApi::new().await;
    remote
        .remote
        .responses
        .lock()
        .await
        .push_back((StatusCode::INTERNAL_SERVER_ERROR, json!({})));
    remote
        .worker(&fixture)
        .provision(&summary(&bot))
        .await
        .expect("retain unknown");
    remote.remote.responses.lock().await.push_back((
        StatusCode::OK,
        json!({"ok":true,"channels":[],"response_metadata":{"next_cursor":""}}),
    ));
    remote
        .worker(&fixture)
        .provision(&summary(&bot))
        .await
        .expect("absence does not recreate");
    remote.remote.responses.lock().await.push_back((
        StatusCode::OK,
        json!({"ok":true,"channels":[],"response_metadata":{"next_cursor":"second"}}),
    ));
    remote
        .worker(&fixture)
        .provision(&summary(&bot))
        .await
        .expect("recover and invite");
    let requests = remote.remote.requests.lock().await;
    assert_eq!(
        requests
            .iter()
            .filter(|p| *p == "/conversations.create")
            .count(),
        1
    );
    assert!(requests.iter().any(|p| p.contains("cursor=second")));
    assert!(requests.iter().any(|path| path == "/conversations.invite"));
    assert_eq!(remote.remote.channel.lock().await["name"], "news");
    drop(requests);
    assert!(
        fixture
            .worker
            .store
            .dedicated_channel("CNEWS")
            .await
            .expect("bound")
    );
    remote.stop().await;
    fixture.stop().await;
}

#[tokio::test]
async fn missing_scope_is_visible_and_a_lost_invite_can_resume_without_rebinding() {
    let fixture = Fixture::new().await;
    let bot = super::agents::news_bot(&fixture).await;
    let remote = TestApi::new().await;
    remote
        .remote
        .responses
        .lock()
        .await
        .push_back((StatusCode::OK, json!({"ok":false,"error":"missing_scope"})));
    remote
        .worker(&fixture)
        .provision(&summary(&bot))
        .await
        .expect("scope failure retained");
    assert!(
        fixture
            .worker
            .store
            .channel_description(bot.id.to_string())
            .await
            .expect("status")
            .contains("reinstall")
    );
    assert!(
        !fixture
            .worker
            .store
            .dedicated_channel("CNEWS")
            .await
            .expect("not bound")
    );
    let channel = remote.remote.channel.lock().await.clone();
    remote.remote.responses.lock().await.extend([
        (StatusCode::OK, json!({"ok":true,"channel":channel})),
        (StatusCode::INTERNAL_SERVER_ERROR, json!({})),
    ]);
    remote
        .worker(&fixture)
        .provision(&summary(&bot))
        .await
        .expect("invite unknown");
    remote.remote.responses.lock().await.push_back((
        StatusCode::OK,
        json!({"ok":false,"error":"already_in_channel"}),
    ));
    remote
        .worker(&fixture)
        .provision(&summary(&bot))
        .await
        .expect("invitation replay");
    assert!(
        fixture
            .worker
            .store
            .channel_description(bot.id.to_string())
            .await
            .expect("ready")
            .ends_with("ready")
    );
    assert_eq!(
        remote
            .remote
            .requests
            .lock()
            .await
            .iter()
            .filter(|p| *p == "/conversations.create")
            .count(),
        2
    );
    remote.stop().await;
    fixture.stop().await;
}

fn summary(bot: &renoa_local::BotRecord) -> renoa_local::BotSummary {
    renoa_local::BotSummary {
        id: bot.id,
        name: bot.recipe.name.clone(),
        created_by: bot.created_by,
    }
}

#[tokio::test]
async fn dedication_committed_after_selection_lookup_cannot_switch_the_channel_agent() {
    let fixture = Fixture::new().await;
    let bot = super::agents::news_bot(&fixture).await;
    let stale = fixture
        .receiver
        .select_agent("!agent arcee")
        .await
        .expect("selection before provisioning");
    let remote = TestApi::new().await;
    remote
        .worker(&fixture)
        .provision(&summary(&bot))
        .await
        .expect("dedicate between lookup and admission");
    fixture
        .worker
        .store
        .admit_with_agent(
            crate::ingress::Incoming {
                event_id: "Erace".to_owned(),
                topic: crate::ingress::Topic {
                    channel: "CNEWS".to_owned(),
                    thread: "1.000001".to_owned(),
                },
                message_ts: "1.000001".to_owned(),
                text: "!agent arcee".to_owned(),
                starts_conversation: true,
            },
            1,
            stale,
        )
        .await
        .expect("admit using stale selection");
    let work = fixture
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("notice");
    assert!(matches!(work.command, crate::commands::Command::Notice(_)));
    assert_eq!(
        fixture
            .worker
            .store
            .session_agent(work.session_id)
            .await
            .expect("bound target")
            .to_string(),
        bot.id.to_string()
    );
    remote.stop().await;
    fixture.stop().await;
}

#[tokio::test]
async fn readable_names_handle_collisions_and_lost_rename_responses_on_the_same_channel() {
    let fixture = Fixture::new().await;
    let bot = super::agents::news_bot(&fixture).await;
    let remote = TestApi::new().await;
    let worker = remote.worker(&fixture);
    worker
        .provision(&summary(&bot))
        .await
        .expect("initial readable name");
    assert_eq!(remote.remote.channel.lock().await["name"], "news");
    let before: Vec<(String, String)> = fixture
        .worker
        .store
        .run(|db| {
            let mut q =
                db.prepare("SELECT channel,session_id FROM conversations WHERE channel='CNEWS'")?;
            Ok(q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .expect("binding");
    let mut renamed = summary(&bot);
    renamed.name = "Daily News".to_owned();
    let current = remote.remote.channel.lock().await.clone();
    remote.remote.responses.lock().await.extend([
        (StatusCode::OK, json!({"ok":true,"channel":current})),
        (StatusCode::OK, json!({"ok":false,"error":"name_taken"})),
    ]);
    worker
        .provision(&renamed)
        .await
        .expect("reserve next available label");
    let current = remote.remote.channel.lock().await.clone();
    remote.remote.responses.lock().await.extend([
        (StatusCode::OK, json!({"ok":true,"channel":current})),
        (StatusCode::INTERNAL_SERVER_ERROR, json!({})),
    ]);
    worker
        .provision(&renamed)
        .await
        .expect("lost successful rename response");
    assert_eq!(remote.remote.channel.lock().await["name"], "daily-news-2");
    worker
        .provision(&renamed)
        .await
        .expect("reconcile exact channel by ID");
    assert!(
        fixture
            .worker
            .store
            .channel_description(bot.id.to_string())
            .await
            .expect("label")
            .starts_with("#daily-news-2:")
    );
    let after: Vec<(String, String)> = fixture
        .worker
        .store
        .run(|db| {
            let mut q =
                db.prepare("SELECT channel,session_id FROM conversations WHERE channel='CNEWS'")?;
            Ok(q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .expect("binding retained");
    assert_eq!(before, after);
    assert_eq!(
        remote
            .remote
            .requests
            .lock()
            .await
            .iter()
            .filter(|r| *r == "/conversations.create")
            .count(),
        1
    );
    remote.stop().await;
    fixture.stop().await;
}
