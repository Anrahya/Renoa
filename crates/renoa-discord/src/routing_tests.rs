//! Which agent a gateway message reaches, and the place it is submitted with.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde_json::{Value, json};
use tokio::sync::Notify;
use uuid::Uuid;

use super::{Inbox, accept};
use crate::{
    api::DiscordApi,
    control::DiscordBindingRequest,
    places,
    snowflake::Snowflake,
    store::{QueuedTurn, SurfaceStore},
};

const BOT: &str = "900";

/// Discord knows one thread the gateway never described: 304 in #desk.
async fn channel(
    State(lookups): State<Arc<AtomicUsize>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    lookups.fetch_add(1, Ordering::SeqCst);
    match id.as_str() {
        "304" => Ok(Json(json!({
            "id": "304", "guild_id": "10", "type": 11, "name": "late", "parent_id": "202"
        }))),
        _ => Err(StatusCode::NOT_FOUND),
    }
}

fn message(id: &str, channel: &str, guild: Option<&str>, mention: bool) -> Vec<u8> {
    let mut message = json!({
        "id": id,
        "channel_id": channel,
        "content": if mention { format!("<@{BOT}> hello") } else { "hello".to_owned() },
        "author": {"id": "20"},
        "mentions": if mention { json!([{"id": BOT}]) } else { json!([]) },
    });
    if let Some(guild) = guild {
        message["guild_id"] = json!(guild);
    }
    serde_json::to_vec(&message).expect("payload")
}

/// The turn one message queued, answered so the next one is visible.
async fn turn(inbox: &Inbox<'_>, payload: &[u8]) -> Option<QueuedTurn> {
    accept(inbox, Some(BOT), payload).await.expect("accept");
    let queued = inbox.store.next_queued().expect("queue")?;
    inbox
        .store
        .answer_locally(&queued.message_id, "done")
        .expect("answer");
    Some(queued)
}

struct Fixture {
    _files: tempfile::TempDir,
    store: SurfaceStore,
    api: DiscordApi,
    wake: Notify,
    guild_id: Snowflake,
    operator: Snowflake,
    lookups: Arc<AtomicUsize>,
    default: Uuid,
    desk: Uuid,
}

impl Fixture {
    /// #desk (202) is bound and #lounge (505) is not; the gateway described
    /// both and one thread in each.
    async fn new() -> Self {
        let files = tempfile::tempdir().expect("files");
        let store = SurfaceStore::open(files.path()).expect("store");
        let (default, desk) = (Uuid::new_v4(), Uuid::new_v4());
        let snowflake = |value: &str| Snowflake::parse(value).expect("snowflake");
        store
            .bind_identity(&snowflake("10"), &snowflake("20"), default)
            .expect("identity");
        let binding = DiscordBindingRequest {
            operation_id: Uuid::new_v4(),
            channel_id: "202".into(),
            agent_id: desk,
            expected_revision: 0,
        };
        store.bind_channel(&binding, "desk").expect("bind #desk");
        let guild = json!({
            "id": "10",
            "channels": [
                {"id": "202", "type": 0, "name": "desk"},
                {"id": "505", "type": 0, "name": "lounge"}
            ],
            "threads": [
                {"id": "303", "type": 11, "name": "plan", "parent_id": "202"},
                {"id": "606", "type": 11, "name": "chat", "parent_id": "505"}
            ]
        });
        store
            .apply_places(
                &places::changes("GUILD_CREATE", &guild, "10")
                    .expect("guild")
                    .expect("ours"),
            )
            .expect("directory");
        let lookups = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let router = Router::new()
            .route("/channels/{id}", get(channel))
            .with_state(Arc::clone(&lookups));
        tokio::spawn(async move { axum::serve(listener, router).await.expect("serve") });
        Self {
            _files: files,
            store,
            api: DiscordApi::with_origin("token".into(), origin).expect("api"),
            wake: Notify::new(),
            guild_id: snowflake("10"),
            operator: snowflake("20"),
            lookups,
            default,
            desk,
        }
    }

    fn inbox(&self) -> Inbox<'_> {
        Inbox {
            store: &self.store,
            api: &self.api,
            wake: &self.wake,
            guild_id: &self.guild_id,
            operator_user_id: &self.operator,
        }
    }

    fn lookups(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn a_bound_channel_and_its_threads_reach_its_agent_with_where_they_were_written() {
    let fixture = Fixture::new().await;
    let inbox = fixture.inbox();

    let bound = turn(&inbox, &message("1001", "202", Some("10"), false))
        .await
        .expect("a bound channel answers without a mention");
    assert_eq!(bound.agent_id, fixture.desk);
    assert_eq!(
        bound.context.as_deref(),
        Some("Discord server 10\nchannel #desk (202)")
    );

    let thread = turn(&inbox, &message("1002", "303", Some("10"), false))
        .await
        .expect("a thread of a bound channel answers without a mention");
    assert_eq!(
        thread.agent_id, fixture.desk,
        "the thread answers as #desk's agent"
    );
    assert_ne!(thread.task_id, bound.task_id, "each thread is its own task");
    assert_eq!(
        thread.context.as_deref(),
        Some("Discord server 10\nchannel #desk (202)\nthread \"plan\" (303)")
    );

    let unseen = turn(&inbox, &message("1003", "304", Some("10"), false))
        .await
        .expect("an unseen thread is looked up");
    assert_eq!(unseen.agent_id, fixture.desk);
    assert_eq!(
        unseen.context.as_deref(),
        Some("Discord server 10\nchannel #desk (202)\nthread \"late\" (304)")
    );
    turn(&inbox, &message("1004", "304", Some("10"), false))
        .await
        .expect("again");
    assert_eq!(fixture.lookups(), 1, "Discord is asked once");
}

#[tokio::test]
async fn unbound_unknown_and_direct_messages_reach_the_default_agent() {
    let fixture = Fixture::new().await;
    let inbox = fixture.inbox();

    assert!(
        turn(&inbox, &message("1005", "505", Some("10"), false))
            .await
            .is_none(),
        "an unbound channel needs a mention"
    );
    let unbound = turn(&inbox, &message("1006", "505", Some("10"), true))
        .await
        .expect("mentioned");
    assert_eq!(unbound.agent_id, fixture.default);
    assert_eq!(
        unbound.context.as_deref(),
        Some("Discord server 10\nchannel #lounge (505)")
    );
    let unbound_thread = turn(&inbox, &message("1007", "606", Some("10"), true))
        .await
        .expect("mentioned in a thread");
    assert_eq!(unbound_thread.agent_id, fixture.default);

    let unknown = turn(&inbox, &message("1008", "808", Some("10"), true))
        .await
        .expect("a channel Discord cannot describe still answers");
    assert_eq!(unknown.agent_id, fixture.default);
    assert_eq!(
        unknown.context.as_deref(),
        Some("Discord server 10\nchannel 808")
    );

    let direct = turn(&inbox, &message("1009", "707", None, false))
        .await
        .expect("the operator's direct message");
    assert_eq!(direct.agent_id, fixture.default);
    assert_eq!(
        direct.context.as_deref(),
        Some("Discord direct message (channel 707)")
    );
    assert_eq!(
        fixture.lookups(),
        1,
        "only the unseen server channel was looked up"
    );
}
