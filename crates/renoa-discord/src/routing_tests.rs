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
use renoa_protocol::Author;
use serde_json::{Value, json};
use tokio::sync::Notify;
use uuid::Uuid;

use super::{Inbox, Wake, accept};
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

/// A message from the operator, Yash.
fn message(id: &str, channel: &str, guild: Option<&str>, mention: bool) -> Vec<u8> {
    let mut message: Value =
        serde_json::from_slice(&guest_message(id, channel, guild, mention)).expect("message");
    message["author"] = json!({"id": "20", "username": "yash", "global_name": "Yash"});
    message.as_object_mut().expect("object").remove("member");
    serde_json::to_vec(&message).expect("payload")
}

/// A message from Mira, a guest in the server, under her server nickname.
fn guest_message(id: &str, channel: &str, guild: Option<&str>, mention: bool) -> Vec<u8> {
    let mut message = json!({
        "id": id,
        "channel_id": channel,
        "content": if mention { format!("<@{BOT}> hello") } else { "hello".to_owned() },
        "author": {"id": "30", "username": "mira_k", "global_name": "Mira K"},
        "member": {"nick": "Mira"},
        "mentions": if mention { json!([{"id": BOT}]) } else { json!([]) },
    });
    if let Some(guild) = guild {
        message["guild_id"] = json!(guild);
    }
    // Discord numbers the messages of a thread.
    if ["303", "304", "606"].contains(&channel) {
        message["position"] = json!(1);
    }
    serde_json::to_vec(&message).expect("payload")
}

/// The turn one message queued, answered so the next one is visible.
async fn turn(inbox: &mut Inbox<'_>, payload: &[u8]) -> Option<QueuedTurn> {
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
    replies: Notify,
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
                &places::changes("GUILD_CREATE", &guild, &snowflake("10"))
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
            replies: Notify::new(),
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
            wake: Wake {
                turns: &self.wake,
                replies: &self.replies,
            },
            guild_id: &self.guild_id,
            operator_user_id: &self.operator,
            failed_lookups: std::collections::HashMap::new(),
        }
    }

    fn lookups(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn a_bound_channel_and_its_threads_reach_its_agent_with_where_they_were_written() {
    let fixture = Fixture::new().await;
    let mut inbox = fixture.inbox();

    let bound = turn(&mut inbox, &message("1001", "202", Some("10"), false))
        .await
        .expect("a bound channel answers without a mention");
    assert_eq!(bound.agent_id, fixture.desk);
    assert_eq!(bound.author, Author::Principal, "the operator is the owner");
    assert_eq!(
        bound.context.as_deref(),
        Some("Discord server 10\nchannel #desk (202)\nfrom Yash (owner)")
    );

    let thread = turn(&mut inbox, &message("1002", "303", Some("10"), false))
        .await
        .expect("a thread of a bound channel answers without a mention");
    assert_eq!(
        thread.agent_id, fixture.desk,
        "the thread answers as #desk's agent"
    );
    assert_ne!(thread.task_id, bound.task_id, "each thread is its own task");
    assert_eq!(
        thread.context.as_deref(),
        Some("Discord server 10\nchannel #desk (202)\nthread \"plan\" (303)\nfrom Yash (owner)")
    );

    let guest = turn(&mut inbox, &guest_message("1010", "202", Some("10"), false))
        .await
        .expect("anyone in a bound channel is answered");
    assert_eq!(guest.agent_id, fixture.desk);
    assert_eq!(guest.author, Author::Guest, "anyone else is a guest");
    assert_eq!(
        guest.context.as_deref(),
        Some("Discord server 10\nchannel #desk (202)\nfrom Mira (guest)")
    );

    let unseen = turn(&mut inbox, &message("1003", "304", Some("10"), false))
        .await
        .expect("an unseen thread is looked up");
    assert_eq!(unseen.agent_id, fixture.desk);
    assert_eq!(
        unseen.context.as_deref(),
        Some("Discord server 10\nchannel #desk (202)\nthread \"late\" (304)\nfrom Yash (owner)")
    );
    turn(&mut inbox, &message("1004", "304", Some("10"), false))
        .await
        .expect("again");
    assert_eq!(fixture.lookups(), 1, "Discord is asked once");
}

#[tokio::test]
async fn unbound_unknown_and_direct_messages_reach_the_default_agent() {
    let fixture = Fixture::new().await;
    let mut inbox = fixture.inbox();

    assert!(
        turn(&mut inbox, &message("1005", "505", Some("10"), false))
            .await
            .is_none(),
        "an unbound channel needs a mention"
    );
    let unbound = turn(&mut inbox, &message("1006", "505", Some("10"), true))
        .await
        .expect("mentioned");
    assert_eq!(unbound.agent_id, fixture.default);
    assert_eq!(
        unbound.context.as_deref(),
        Some("Discord server 10\nchannel #lounge (505)\nfrom Yash (owner)")
    );
    let unbound_thread = turn(&mut inbox, &message("1007", "606", Some("10"), true))
        .await
        .expect("mentioned in a thread");
    assert_eq!(unbound_thread.agent_id, fixture.default);

    let unknown = turn(&mut inbox, &message("1008", "808", Some("10"), true))
        .await
        .expect("a channel Discord cannot describe still answers");
    assert_eq!(unknown.agent_id, fixture.default);
    assert_eq!(
        unknown.context.as_deref(),
        Some("Discord server 10\nchannel 808\nfrom Yash (owner)")
    );
    turn(&mut inbox, &message("1010", "808", Some("10"), true))
        .await
        .expect("mentioned again");
    assert!(
        turn(&mut inbox, &message("1011", "909", Some("10"), false))
            .await
            .is_none(),
        "unaddressed chatter in an unseen channel is not looked up"
    );

    let direct = turn(&mut inbox, &message("1009", "707", None, false))
        .await
        .expect("the operator's direct message");
    assert_eq!(direct.agent_id, fixture.default);
    assert_eq!(
        direct.context.as_deref(),
        Some("Discord direct message (channel 707)\nfrom Yash (owner)")
    );
    assert_eq!(
        fixture.lookups(),
        1,
        "only the unseen server channel was looked up"
    );
}

#[tokio::test]
async fn a_failed_lookup_is_retried_after_a_minute_and_then_forgotten() {
    let fixture = Fixture::new().await;
    let mut inbox = fixture.inbox();
    let stale = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(61))
        .expect("a minute ago");
    let snowflake = |value: &str| Snowflake::parse(value).expect("snowflake");
    inbox.failed_lookups.insert(snowflake("808"), stale);
    inbox.failed_lookups.insert(snowflake("909"), stale);

    turn(&mut inbox, &message("1001", "808", Some("10"), true))
        .await
        .expect("mentioned");
    assert_eq!(
        fixture.lookups(),
        1,
        "an expired failure is asked about again"
    );
    assert_eq!(
        inbox.failed_lookups.keys().collect::<Vec<_>>(),
        [&snowflake("808")],
        "expired failures are dropped"
    );
}

/// The same message with other text.
fn saying(payload: &[u8], content: &str) -> Vec<u8> {
    let mut message: Value = serde_json::from_slice(payload).expect("message");
    message["content"] = json!(content);
    serde_json::to_vec(&message).expect("payload")
}

/// The turn one message queued, marked sent so the next one is visible.
async fn submitted(inbox: &mut Inbox<'_>, payload: &[u8]) -> Option<QueuedTurn> {
    accept(inbox, Some(BOT), payload).await.expect("accept");
    let queued = inbox.store.next_queued().expect("queue")?;
    inbox
        .store
        .mark_submitted(&queued.message_id)
        .expect("submitted");
    Some(queued)
}

#[tokio::test]
async fn the_operators_new_starts_a_new_conversation_with_the_same_agent() {
    let fixture = Fixture::new().await;
    let mut inbox = fixture.inbox();
    let first = submitted(&mut inbox, &message("1101", "606", Some("10"), true))
        .await
        .expect("mentioned in a thread");

    let new = saying(&message("1102", "606", Some("10"), false), "/new");
    for _ in 0..2 {
        assert!(
            submitted(&mut inbox, &new).await.is_none(),
            "/new runs no turn"
        );
    }
    let reply = inbox
        .store
        .next_outbound()
        .expect("outbound")
        .expect("/new is answered");
    assert_eq!(
        (
            reply.channel_id.as_str(),
            reply.reply_to.as_deref(),
            reply.body.as_str()
        ),
        ("606", Some("1102"), "Started a new conversation.")
    );
    inbox
        .store
        .mark_sending(&reply.command_id, reply.chunk)
        .expect("sending");
    assert!(
        inbox.store.next_outbound().expect("outbound").is_none(),
        "a redelivered /new is answered once"
    );

    let next = submitted(&mut inbox, &message("1103", "606", Some("10"), false))
        .await
        .expect("the thread still answers without a mention");
    assert_ne!(next.task_id, first.task_id, "a new task");
    assert_eq!(next.agent_id, first.agent_id, "with the same agent");
    let later = submitted(&mut inbox, &message("1104", "606", Some("10"), false))
        .await
        .expect("continues");
    assert_eq!(
        later.task_id, next.task_id,
        "the new conversation continues"
    );

    let guest = submitted(
        &mut inbox,
        &saying(&guest_message("1105", "606", Some("10"), false), "/new"),
    )
    .await
    .expect("a guest's /new is an ordinary message");
    assert_eq!(
        (guest.prompt.as_str(), guest.author, guest.task_id),
        ("/new", Author::Guest, next.task_id)
    );
}

#[tokio::test]
async fn a_new_conversation_leaves_queued_work_and_other_text_alone() {
    let fixture = Fixture::new().await;
    let mut inbox = fixture.inbox();
    let mention = |id: &str, text: &str| saying(&message(id, "505", Some("10"), true), text);

    accept(&mut inbox, Some(BOT), &mention("1201", "<@900> first"))
        .await
        .expect("queued");
    accept(&mut inbox, Some(BOT), &mention("1202", "<@900>  /NEW "))
        .await
        .expect("/new");
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        fixture.replies.notified(),
    )
    .await
    .expect("reply delivery is woken");
    let queued = inbox
        .store
        .next_queued()
        .expect("queue")
        .expect("the earlier message is still queued");
    assert_eq!(queued.message_id, "1201");
    inbox
        .store
        .mark_submitted(&queued.message_id)
        .expect("submitted");

    let more = submitted(&mut inbox, &mention("1203", "<@900> /new please"))
        .await
        .expect("/new with more text is an ordinary message");
    assert_eq!(more.prompt, "/new please");
    assert_ne!(
        more.task_id, queued.task_id,
        "it starts the new conversation"
    );

    assert!(
        submitted(
            &mut inbox,
            &saying(&message("1204", "707", None, false), "/new")
        )
        .await
        .is_none(),
        "a direct message's /new runs no turn"
    );
}
