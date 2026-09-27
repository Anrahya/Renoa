use super::*;
use axum::{
    Json, Router,
    extract::{Path as RequestPath, State},
    routing::get,
};
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, LocalHost, LocalHostAdapters,
    LocalModelConfiguration, ModelProvider,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn channel_validation_precedes_storage_and_committed_retries_need_no_discord_access() {
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
    let router = Router::new().route("/channels/{id}", get(|RequestPath(id): RequestPath<String>, State(calls): State<Arc<AtomicUsize>>| async move {
        calls.fetch_add(1, Ordering::SeqCst);
        Json(json!({"id":id,"guild_id":if id == "201" {"99"} else {"10"},"type":if id == "203" {2} else {0},"name":"desk"}))
    })).with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let control = DiscordControl {
        home: home.clone(),
        guild: Snowflake::parse("10").unwrap(),
        operator: Snowflake::parse("20").unwrap(),
        default_agent: Uuid::parse_str(&agent.id.to_string()).unwrap(),
        api: Arc::new(
            DiscordApi::with_origin("fixture".into(), format!("http://{address}")).unwrap(),
        ),
    };
    let request = DiscordBindingRequest {
        operation_id: Uuid::new_v4(),
        channel_id: "202".into(),
        agent_id: control.default_agent,
        expected_revision: 0,
    };
    assert!(control.bindings().unwrap().is_empty());
    assert!(!home.join("state/surfaces/discord").exists());
    let mut invalid = request.clone();
    invalid.agent_id = Uuid::new_v4();
    assert!(control.bind(&host, invalid).await.is_err());
    let mut invalid = request.clone();
    invalid.expected_revision = 1;
    assert!(control.bind(&host, invalid).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for channel in ["201", "203"] {
        let mut invalid = request.clone();
        invalid.channel_id = channel.into();
        assert!(control.bind(&host, invalid).await.is_err());
        assert!(!home.join("state/surfaces/discord").exists());
    }
    let saved = control.bind(&host, request.clone()).await.unwrap();
    assert_eq!(saved.channel_id, "202");
    assert_eq!(saved.revision, 1);
    assert_eq!(control.bindings().unwrap(), vec![saved.clone()]);
    task.abort();
    assert_eq!(control.bind(&host, request.clone()).await.unwrap(), saved);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let mut conflict = request;
    conflict.expected_revision = 1;
    assert!(control.bind(&host, conflict).await.is_err());
    assert_eq!(control.bindings().unwrap(), vec![saved]);
}
