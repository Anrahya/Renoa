use super::{End, SocketState, Step, close_end, prepare_reconnect};
use crate::store::{GatewayCursor, SurfaceStore};

fn closed_with(code: u16) -> End {
    let frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
        code: code.into(),
        reason: "test".into(),
    };
    close_end(Some(&frame))
}

#[test]
fn identify_requests_content_for_unmentioned_follow_ups() {
    assert_ne!(super::INTENTS & (1 << 15), 0);
}

#[test]
fn expired_resume_state_starts_a_fresh_session() {
    assert!(matches!(closed_with(4007), End::Reconnect { fresh: true }));
    assert!(matches!(closed_with(4009), End::Reconnect { fresh: true }));
}

#[test]
fn a_fresh_reconnect_forgets_the_durable_resume_cursor() {
    let directory = tempfile::tempdir().expect("temp directory");
    let store = SurfaceStore::open(directory.path()).expect("store");
    store
        .save_gateway(GatewayCursor {
            session_id: Some("session".to_owned()),
            resume_url: Some("wss://gateway.discord.gg".to_owned()),
            sequence: Some(42),
        })
        .expect("save cursor");

    assert!(!prepare_reconnect(&store, false).expect("resumable reconnect"));
    assert_eq!(
        store.load_gateway().expect("preserved cursor").sequence,
        Some(42)
    );
    assert!(prepare_reconnect(&store, true).expect("fresh reconnect"));
    let cursor = store.load_gateway().expect("cleared cursor");
    assert!(cursor.session_id.is_none());
    assert!(cursor.resume_url.is_none());
    assert!(cursor.sequence.is_none());
}

#[test]
fn a_disallowed_message_content_intent_explains_the_required_setup() {
    let End::Failed(error) = closed_with(4014) else {
        panic!("a disallowed intent must stop the gateway");
    };
    assert!(error.to_string().contains("Message Content"), "{error}");
}

#[test]
fn hello_identifies_and_a_mention_payload_is_exposed() {
    let mut state = SocketState::new(None, None, None);
    let hello = state
        .receive(r#"{"op":10,"d":{"heartbeat_interval":1000}}"#, "token")
        .expect("hello");
    let Step::Send(frame) = hello else {
        panic!("hello did not identify");
    };
    assert_eq!(frame["op"], 2);
    assert_eq!(frame["d"]["intents"], super::INTENTS);

    let ready = state
        .receive(
            r#"{"op":0,"s":1,"t":"READY","d":{"session_id":"sess","resume_gateway_url":"wss://gateway.discord.gg","user":{"id":"50"}}}"#,
            "token",
        )
        .expect("ready");
    assert!(matches!(ready, Step::Ready));
    assert_eq!(state.bot_user_id.as_deref(), Some("50"));
    assert_eq!(state.sequence, Some(1));

    let message = state
        .receive(
            r#"{"op":0,"s":2,"t":"MESSAGE_CREATE","d":{"id":"101","content":"hi"}}"#,
            "token",
        )
        .expect("message");
    let Step::Message(payload) = message else {
        panic!("message was not exposed");
    };
    let payload: serde_json::Value = serde_json::from_slice(&payload).expect("payload");
    assert_eq!(payload["id"], "101");
}

#[test]
fn an_invalid_session_drops_resume_state() {
    let mut state = SocketState::new(
        Some("sess".to_owned()),
        Some("wss://gateway.discord.gg".to_owned()),
        Some(4),
    );
    let step = state
        .receive(r#"{"op":9,"d":false}"#, "token")
        .expect("invalid");
    assert!(matches!(step, Step::Reconnect { fresh: true }));
    assert!(state.session_id.is_none());
    assert!(state.resume_url.is_none());
    assert!(state.sequence.is_none());
}

#[test]
fn a_heartbeat_ack_is_not_another_heartbeat() {
    let mut state = SocketState::new(None, None, Some(3));
    let step = state.receive(r#"{"op":11}"#, "token").expect("ack");
    assert!(matches!(step, Step::Ack));
    assert_eq!(state.sequence, Some(3));
}
