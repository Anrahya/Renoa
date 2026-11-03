use std::sync::{Arc, Mutex};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use renoa_agent::{AgentEvent, AgentEventSink as _, ContentBlock, ToolCall, ToolOutput};
use renoa_control::{
    ClientMessage, DeviceId, DeviceNoticeKey, ErrorCode, MAX_SEALED_NOTICE_BYTES, NoticeKey,
    SealedNotice,
};
use renoa_local::{RenoaHome, SetupLink};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    Route, SetupLinks, notice_id, plaintext, seal,
    seal::tests::{device_key, open},
};
use crate::agent_link::{AgentLink, Answer};

const URL: &str = "https://provider.example/authorize?state=one";

/// What the fake coordinator was asked to hold and withdraw.
#[derive(Default)]
struct Held {
    puts: Vec<SealedNotice>,
    withdrawn: Vec<Uuid>,
}

/// A coordinator session that lists `keys` and accepts every notice.
fn coordinator(keys: Vec<DeviceNoticeKey>) -> (Arc<AgentLink>, Arc<Mutex<Held>>) {
    coordinator_that(keys, true)
}

/// A coordinator session that lists `keys` and accepts or refuses every put.
fn coordinator_that(
    keys: Vec<DeviceNoticeKey>,
    accepts: bool,
) -> (Arc<AgentLink>, Arc<Mutex<Held>>) {
    let link = Arc::new(AgentLink::default());
    let mut session = link.open();
    let held = Arc::new(Mutex::new(Held::default()));
    let log = Arc::clone(&held);
    tokio::spawn(async move {
        loop {
            let (request_id, answer) = match session.next_message().await {
                ClientMessage::ListNoticeKeys { request_id } => {
                    (request_id, Answer::NoticeKeys(keys.clone()))
                }
                ClientMessage::PutNotice { request_id, notice } => {
                    log.lock().unwrap().puts.push(notice);
                    let answer = if accepts {
                        Answer::Accepted
                    } else {
                        Answer::Refused {
                            code: ErrorCode::InvalidRole,
                            message: "list the notice keys again".to_owned(),
                        }
                    };
                    (request_id, answer)
                }
                ClientMessage::WithdrawNotices {
                    request_id,
                    notice_ids,
                } => {
                    log.lock().unwrap().withdrawn.extend(notice_ids);
                    (request_id, Answer::Accepted)
                }
                other => panic!("unexpected request {other:?}"),
            };
            session.answer(request_id, answer).unwrap();
        }
    });
    (link, held)
}

fn home() -> (tempfile::TempDir, RenoaHome) {
    let files = tempfile::tempdir().unwrap();
    let home = RenoaHome::at(files.path().join("home")).unwrap();
    home.initialize().unwrap();
    (files, home)
}

fn authorization(url: &str) -> Value {
    json!({
        "status": "authorization_required",
        "connection": "plugin.notion.default",
        "display_name": "notion-mcp",
        "authorization_url": url,
        "expires_at_ms": i64::MAX,
        "message": "Open it"
    })
}

/// A tool call starts and shows `progress`, as the agent loop reports it.
async fn show(links: &SetupLinks, tool: &str, call: &str, progress: &Value) {
    let call = ToolCall {
        id: call.to_owned(),
        name: tool.to_owned(),
        arguments: json!({}),
        thought_signature: None,
        namespace: None,
    };
    links
        .emit(AgentEvent::ToolExecutionStart { call: call.clone() })
        .await;
    links
        .emit(AgentEvent::ToolExecutionUpdate {
            call,
            update: ToolOutput {
                content: vec![ContentBlock::text(progress.to_string())],
                details: None,
                is_error: false,
                closing: None,
            },
        })
        .await;
}

/// `progress`'s link, as the shared check passes it.
fn checked(progress: &Value) -> SetupLink {
    let update = ToolOutput {
        content: vec![ContentBlock::text(progress.to_string())],
        details: None,
        is_error: false,
        closing: None,
    };
    SetupLink::from_update("plugin_manage", &update)
        .expect("setup progress")
        .expect("a valid link")
}

fn app(link: &Arc<AgentLink>, home: &RenoaHome, command: Uuid) -> SetupLinks {
    SetupLinks::open(home, link, "android", command, CancellationToken::new())
}

#[tokio::test]
async fn an_app_command_seals_its_link_to_each_signed_in_device() {
    let (private, public) = device_key();
    let device = DeviceId::new();
    let keys = vec![
        DeviceNoticeKey {
            device_id: device,
            public_key: NoticeKey::new(URL_SAFE_NO_PAD.encode(public)),
        },
        DeviceNoticeKey {
            device_id: DeviceId::new(),
            public_key: NoticeKey::new("not-a-key"),
        },
    ];
    let (link, held) = coordinator(keys);
    let (_files, home) = home();
    let command = Uuid::new_v4();
    let links = app(&link, &home, command);
    show(&links, "plugin_manage", "call-1", &authorization(URL)).await;
    show(&links, "plugin_manage", "call-1", &authorization(URL)).await;
    show(&links, "other_tool", "call-2", &authorization(URL)).await;
    assert!(links.finish().await.is_none(), "the link reached the app");

    let held = held.lock().unwrap();
    assert!(held.withdrawn.is_empty());
    let [first, again] = held.puts.as_slice() else {
        panic!(
            "one notice per showing for the keyed device: {:?}",
            held.puts
        );
    };
    let id = notice_id(
        command,
        "call-1",
        &checked(&authorization(URL)),
        device.as_uuid(),
    );
    assert_eq!(
        (first.notice_id, again.notice_id),
        (id, id),
        "a retry converges"
    );
    assert_ne!(
        id,
        notice_id(
            command,
            "call-1",
            &checked(&authorization(
                "https://provider.example/authorize?state=two"
            )),
            device.as_uuid()
        ),
        "another link of the same call is another notice"
    );
    assert_eq!(first.device_id, device);
    assert_eq!(first.expires_at_ms, i64::MAX);
    assert!(!first.sealed.contains("provider.example"));
    let opened = open(&private, &first.sealed, id, device.as_uuid()).expect("the device opens it");
    assert_eq!(
        serde_json::from_slice::<Value>(&opened).unwrap(),
        json!({
            "kind": "authorization",
            "label": "Notion MCP",
            "url": URL,
            "expires_at_ms": i64::MAX,
        })
    );
}

#[tokio::test]
async fn a_link_no_app_can_receive_stops_the_turn() {
    let (link, held) = coordinator(Vec::new());
    let (_files, home) = home();
    let cancellation = CancellationToken::new();
    let links = SetupLinks::open(
        &home,
        &link,
        "android",
        Uuid::new_v4(),
        cancellation.clone(),
    );
    show(&links, "plugin_manage", "call-1", &authorization(URL)).await;
    assert!(cancellation.is_cancelled());
    let stopped = links.finish().await.expect("stopped");
    assert!(
        stopped
            .reason
            .as_deref()
            .unwrap()
            .contains("No signed-in Renoa app"),
        "{:?}",
        stopped.reason
    );
    assert_eq!(stopped.calls, ["call-1"]);
    assert!(held.lock().unwrap().puts.is_empty());
}

#[tokio::test]
async fn an_invalid_link_stops_the_turn_and_keeps_the_links_that_reached_the_app() {
    let (_, public) = device_key();
    let keys = vec![DeviceNoticeKey {
        device_id: DeviceId::new(),
        public_key: NoticeKey::new(URL_SAFE_NO_PAD.encode(public)),
    }];
    let (link, held) = coordinator(keys);
    let (_files, home) = home();
    let links = app(&link, &home, Uuid::new_v4());
    show(&links, "plugin_manage", "good", &authorization(URL)).await;
    show(
        &links,
        "plugin_manage",
        "bad",
        &authorization("http://provider.example/authorize"),
    )
    .await;
    let stopped = links.finish().await.expect("stopped");
    assert!(
        stopped
            .reason
            .as_deref()
            .unwrap()
            .contains("invalid setup link"),
        "{:?}",
        stopped.reason
    );
    assert_eq!(stopped.calls, ["bad"]);
    let held = held.lock().unwrap();
    assert_eq!(held.puts.len(), 1);
    assert!(held.withdrawn.is_empty(), "the delivered link stays");
}

#[tokio::test]
async fn a_link_every_device_refused_is_withdrawn_and_its_setup_forgotten() {
    let (_, public) = device_key();
    let keys = vec![DeviceNoticeKey {
        device_id: DeviceId::new(),
        public_key: NoticeKey::new(URL_SAFE_NO_PAD.encode(public)),
    }];
    let (link, held) = coordinator_that(keys, false);
    let (_files, home) = home();
    let links = app(&link, &home, Uuid::new_v4());
    show(&links, "plugin_manage", "call-1", &authorization(URL)).await;
    let stopped = links.finish().await.expect("stopped");
    assert!(
        stopped
            .reason
            .as_deref()
            .unwrap()
            .contains("list the notice keys again"),
        "{:?}",
        stopped.reason
    );
    assert_eq!(stopped.calls, ["call-1"]);
    let held = held.lock().unwrap();
    assert_eq!(
        held.withdrawn,
        [held.puts[0].notice_id],
        "its outcome may be unknown"
    );
}

#[tokio::test]
async fn a_turn_stopped_before_its_link_arrived_forgets_the_setup_and_sends_nothing() {
    let (link, held) = coordinator(Vec::new());
    let (_files, home) = home();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let links = SetupLinks::open(&home, &link, "android", Uuid::new_v4(), cancellation);
    show(&links, "plugin_manage", "call-1", &authorization(URL)).await;
    let stopped = links.finish().await.expect("a call to forget");
    assert_eq!(
        stopped.reason, None,
        "the person stopped it; no link failed"
    );
    assert_eq!(stopped.calls, ["call-1"]);
    assert!(held.lock().unwrap().puts.is_empty());
}

#[tokio::test]
async fn a_finished_turn_keeps_every_setup() {
    let (link, _) = coordinator(Vec::new());
    let (_files, home) = home();
    let links = app(&link, &home, Uuid::new_v4());
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "plugin_manage".to_owned(),
        arguments: json!({}),
        thought_signature: None,
        namespace: None,
    };
    links.emit(AgentEvent::ToolExecutionStart { call }).await;
    assert!(links.finish().await.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn discord_commands_reach_discord_only_once_it_is_connected() {
    use std::os::unix::fs::PermissionsExt as _;

    let (link, _) = coordinator(Vec::new());
    let (_files, home) = home();
    let command = Uuid::new_v4();
    let discord = || SetupLinks::open(&home, &link, "discord", command, CancellationToken::new());
    let unconnected = discord();
    assert!(matches!(unconnected.route, Route::Closed(_)));
    show(&unconnected, "plugin_manage", "call-1", &authorization(URL)).await;
    let stopped = unconnected.finish().await.expect("stopped");
    assert!(
        stopped
            .reason
            .as_deref()
            .unwrap()
            .contains("Discord is not connected"),
        "{:?}",
        stopped.reason
    );

    let connection = home.discord_connection();
    std::fs::write(
        &connection,
        serde_json::to_vec(&json!({
            "operation_id": Uuid::new_v4(),
            "bot_name": "Renoa",
            "guild_id": "10",
            "guild_name": "Home",
            "operator_user_id": "20",
            "agent_id": Uuid::new_v4(),
            "bot_token": "discord.token",
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(&connection, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(discord().route, Route::Discord(_)));
}

/// The largest link the shared check passes, a query of backslashes JSON
/// doubles under a label JSON doubles too, seals to exactly the coordinator's
/// bound.
#[test]
fn the_largest_notice_is_the_coordinator_bound() {
    let base = "https://a/?";
    let mut progress = authorization(&format!("{base}{}", "\\".repeat(16 * 1024 - base.len())));
    progress["connection"] = json!("\"".repeat(128));
    progress.as_object_mut().unwrap().remove("display_name");
    let update = ToolOutput {
        content: vec![ContentBlock::text(progress.to_string())],
        details: None,
        is_error: false,
        closing: None,
    };
    let link = SetupLink::from_update("plugin_manage", &update)
        .expect("setup progress")
        .expect("the largest valid link");
    assert_eq!(link.url.as_str().len(), 16 * 1024);
    assert_eq!(link.label.len(), 128);
    let plaintext = plaintext(&link, i64::MAX).unwrap();
    let (_, public) = device_key();
    let sealed = seal::seal(&public, Uuid::new_v4(), Uuid::new_v4(), &plaintext).unwrap();
    assert_eq!(sealed.len(), MAX_SEALED_NOTICE_BYTES);
}
