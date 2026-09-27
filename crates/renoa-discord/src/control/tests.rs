use super::*;
use axum::{
    Json, Router,
    extract::{Path as RequestPath, RawQuery, Request, State},
    http::HeaderMap,
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, LocalHost, LocalHostAdapters,
    LocalModelConfiguration, ModelProvider,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "fixture.bot-token";

struct Fixture {
    _files: tempfile::TempDir,
    home: RenoaHome,
    host: LocalHost,
    agent: Uuid,
    origin: String,
    calls: Arc<AtomicUsize>,
    discord: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let files = tempfile::tempdir().unwrap();
        let home = files.path().join("home");
        let bridge = files.path().join("model.mjs");
        let auth = files.path().join("models.sqlite3");
        std::fs::write(&bridge, crate::live_test::MODEL_BRIDGE).unwrap();
        std::fs::write(&auth, "").unwrap();
        let host = LocalHost::new(
            &home,
            LocalModelConfiguration::new(
                &bridge,
                vec![ModelProvider::Xai],
                ModelProvider::Xai,
                "fixture-model",
                &auth,
            ),
            LocalHostAdapters::default(),
        )
        .unwrap();
        let agent = host
            .create_agent(
                AgentCreator::System {
                    component: "fixture".into(),
                },
                AgentCreationOrigin::Provisioning,
                AgentCreateRequest::new(
                    Uuid::new_v4(),
                    "Desk",
                    "You are the owner-created Desk agent.",
                )
                .with_tools([]),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let router = discord().layer(middleware::from_fn_with_state(Arc::clone(&calls), count));
        let discord = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            home: RenoaHome::at(home).unwrap(),
            _files: files,
            host,
            agent: Uuid::parse_str(&agent.id.to_string()).unwrap(),
            origin,
            calls,
            discord,
        }
    }

    fn control(&self) -> DiscordControl {
        DiscordControl::with_origin(self.home.clone(), self.origin.clone())
    }

    fn connect_request(&self, guild: &str) -> DiscordConnectRequest {
        DiscordConnectRequest {
            operation_id: Uuid::new_v4(),
            bot_token: TOKEN.into(),
            guild_id: guild.into(),
            agent_id: self.agent,
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn copy(request: &DiscordConnectRequest) -> DiscordConnectRequest {
    DiscordConnectRequest {
        operation_id: request.operation_id,
        bot_token: request.bot_token.clone(),
        guild_id: request.guild_id.clone(),
        agent_id: request.agent_id,
    }
}

async fn count(State(calls): State<Arc<AtomicUsize>>, request: Request, next: Next) -> Response {
    calls.fetch_add(1, Ordering::SeqCst);
    next.run(request).await
}

/// 201 servers, so membership needs a second page. Only 1001 lacks Administrator.
fn discord() -> Router {
    Router::new()
        .route(
            "/applications/@me",
            get(|headers: HeaderMap| async move {
                let intent = headers["authorization"] != "Bot no.intent";
                Json(json!({
                    "id": "30",
                    "owner": {"id": "20", "username": "owner"},
                    "team": null,
                    "flags": if intent { 1 << 19 } else { 0 },
                }))
            }),
        )
        .route(
            "/users/@me",
            get(|| async { Json(json!({"id": "40", "username": "Renoa", "bot": true})) }),
        )
        .route(
            "/users/@me/guilds",
            get(|RawQuery(query): RawQuery| async move {
                let query = query.unwrap();
                let query: std::collections::HashMap<_, _> =
                    url::form_urlencoded::parse(query.as_bytes()).collect();
                let after: u64 = query["after"].parse().unwrap();
                let limit: usize = query["limit"].parse().unwrap();
                let page: Vec<Value> = (1000_u64..1201)
                    .filter(|id| *id > after)
                    .take(limit)
                    .map(|id| {
                        let bits = if id == 1001 { "3072" } else { "8" };
                        json!({"id": id.to_string(), "name": format!("Server {id}"), "permissions": bits})
                    })
                    .collect();
                Json(page)
            }),
        )
        .route(
            "/guilds/{id}/channels",
            get(|| async {
                Json(json!([
                    {"id": "406", "type": 0, "name": "tie-b", "position": 3, "parent_id": "300"},
                    {"id": "300", "type": 4, "name": "Later", "position": 1},
                    {"id": "402", "type": 0, "name": "desk", "position": 2, "parent_id": "301"},
                    {"id": "404", "type": 2, "name": "voice", "position": 0},
                    {"id": "401", "type": 5, "name": "news", "position": 0, "parent_id": "300"},
                    {"id": "301", "type": 4, "name": "First", "position": 0},
                    {"id": "405", "type": 0, "name": "tie-a", "position": 3, "parent_id": "300"},
                    {"id": "400", "type": 0, "name": "general", "position": 5},
                    {"id": "403", "type": 0, "name": "ops", "position": 1, "parent_id": "301"},
                ]))
            }),
        )
        .route(
            "/channels/{id}",
            get(|RequestPath(id): RequestPath<String>| async move {
                let guild = if id == "201" { "99" } else { "1000" };
                let kind = if id == "203" { 2 } else { 0 };
                Json(json!({"id": id, "guild_id": guild, "type": kind, "name": "desk"}))
            }),
        )
}

#[tokio::test]
async fn connect_requires_intent_membership_and_administrator_before_saving() {
    let f = Fixture::new().await;
    let control = f.control();
    let status = serde_json::to_value(control.status().unwrap()).unwrap();
    assert_eq!(status, json!({"status": "setup_required"}));

    assert!(control.inspect("Bot fixture".into()).await.is_err());
    assert_eq!(f.calls(), 0, "a malformed token is refused locally");
    let inspection = serde_json::to_value(control.inspect(TOKEN.into()).await.unwrap()).unwrap();
    assert_eq!(inspection["bot_name"], "Renoa");
    assert_eq!(
        inspection["invite_url"],
        "https://discord.com/oauth2/authorize?client_id=30&scope=bot&permissions=8"
    );
    let guilds = inspection["guilds"].as_array().unwrap();
    assert_eq!(guilds.len(), 201);
    assert_eq!(
        guilds[1],
        json!({"id": "1001", "name": "Server 1001", "administrator": false})
    );
    assert_eq!(guilds[200]["id"], "1200");

    let mut no_intent = f.connect_request("1000");
    no_intent.bot_token = "no.intent".into();
    for (request, reason) in [
        (no_intent, "Message Content Intent"),
        (f.connect_request("1001"), "Administrator in Server 1001"),
        (f.connect_request("999"), "Invite the bot"),
    ] {
        let error = control.connect(&f.host, request).await.err().unwrap();
        assert!(error.to_string().contains(reason), "{error}");
    }
    let mut missing_agent = f.connect_request("1000");
    missing_agent.agent_id = Uuid::new_v4();
    let before = f.calls();
    assert!(control.connect(&f.host, missing_agent).await.is_err());
    assert_eq!(
        f.calls(),
        before,
        "a missing agent is refused before Discord"
    );
    assert!(!f.home.discord_connection().exists());

    let request = f.connect_request("1000");
    let connected =
        serde_json::to_value(control.connect(&f.host, copy(&request)).await.unwrap()).unwrap();
    let expected = json!({
        "status": "connected",
        "bot_name": "Renoa",
        "guild_name": "Server 1000",
        "default_agent_id": f.agent,
        "bindings": [],
    });
    assert_eq!(connected, expected);

    f.discord.abort();
    let calls = f.calls();
    let retried = control.connect(&f.host, copy(&request)).await.unwrap();
    assert_eq!(serde_json::to_value(retried).unwrap(), expected);
    let reopened = f.control();
    let retried = reopened.connect(&f.host, request).await.unwrap();
    assert_eq!(serde_json::to_value(retried).unwrap(), expected);
    assert_eq!(f.calls(), calls, "an exact retry needs no Discord access");
    let error = reopened
        .connect(&f.host, f.connect_request("1000"))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("already connected"), "{error}");
}

#[tokio::test]
async fn channels_follow_the_discord_sidebar() {
    let f = Fixture::new().await;
    let control = f.control();
    assert!(
        control.channels().await.is_err(),
        "channels need a connection"
    );
    control
        .connect(&f.host, f.connect_request("1000"))
        .await
        .unwrap();
    let names: Vec<_> = serde_json::to_value(control.channels().await.unwrap())
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|channel| channel["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["general", "ops", "desk", "news", "tie-a", "tie-b"]);
}

#[tokio::test]
async fn channel_validation_precedes_storage_and_committed_retries_need_no_discord_access() {
    let f = Fixture::new().await;
    let control = f.control();
    let request = DiscordBindingRequest {
        operation_id: Uuid::new_v4(),
        channel_id: "202".into(),
        agent_id: f.agent,
        expected_revision: 0,
    };
    let error = control.bind(&f.host, request.clone()).await.err().unwrap();
    assert!(error.to_string().contains("Connect Discord"), "{error}");
    control
        .connect(&f.host, f.connect_request("1000"))
        .await
        .unwrap();
    let store = f.home.path().join("state/surfaces/discord");
    assert!(!store.exists());
    let calls = f.calls();
    let mut invalid = request.clone();
    invalid.agent_id = Uuid::new_v4();
    assert!(control.bind(&f.host, invalid).await.is_err());
    let mut invalid = request.clone();
    invalid.expected_revision = 1;
    assert!(control.bind(&f.host, invalid).await.is_err());
    assert_eq!(f.calls(), calls);
    for channel in ["201", "203"] {
        let mut invalid = request.clone();
        invalid.channel_id = channel.into();
        assert!(control.bind(&f.host, invalid).await.is_err());
        assert!(!store.exists());
    }
    let saved = control.bind(&f.host, request.clone()).await.unwrap();
    assert_eq!(saved.channel_id, "202");
    assert_eq!(saved.revision, 1);
    let DiscordStatus::Connected { bindings, .. } = control.status().unwrap() else {
        panic!("connected");
    };
    assert_eq!(bindings, vec![saved.clone()]);
    f.discord.abort();
    assert_eq!(control.bind(&f.host, request.clone()).await.unwrap(), saved);
    assert_eq!(f.calls(), calls + 3);
    let mut conflict = request;
    conflict.expected_revision = 1;
    assert!(control.bind(&f.host, conflict).await.is_err());
}
