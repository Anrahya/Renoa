use super::{SocketState, Step};

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
}

#[test]
fn a_heartbeat_ack_is_not_another_heartbeat() {
    let mut state = SocketState::new(None, None, Some(3));
    let step = state.receive(r#"{"op":11}"#, "token").expect("ack");
    assert!(matches!(step, Step::Ack));
    assert_eq!(state.sequence, Some(3));
}
