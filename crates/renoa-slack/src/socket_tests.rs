use super::*;
use serde_json::{Value, json};

#[tokio::test]
async fn websocket_ack_follows_commit_and_redelivery_keeps_the_same_request() {
    let directory = tempfile::tempdir().expect("surface directory");
    let store = Store::open(
        directory.path(),
        &crate::store::Binding {
            host_id: uuid::Uuid::nil(),
            agent_id: uuid::Uuid::nil(),
            team: "T1",
            bot: "U2",
            user: "U3",
            workspace: directory.path(),
        },
    )
    .expect("store");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let retained = store.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("connection");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("handshake");
        let mut request_id = None;
        for envelope in ["delivery-1", "delivery-2"] {
            socket.send(Message::Text(json!({"type":"events_api","envelope_id":envelope,"payload":{
                "type":"event_callback","team_id":"T1","api_app_id":"A1","authorizations":[{"team_id":"T1","user_id":"U2","is_bot":true}],"event_id":"Ev1",
                "event":{"type":"message","user":"U3","channel":"D1","channel_type":"im","ts":"1.000001","text":"hello"}
            }}).to_string().into())).await.expect("event");
            loop {
                match socket
                    .next()
                    .await
                    .expect("client frame")
                    .expect("valid frame")
                {
                    Message::Text(ack) => {
                        assert_eq!(
                            serde_json::from_str::<Value>(&ack).expect("ack")["envelope_id"],
                            envelope
                        );
                        let work = retained
                            .next_work()
                            .await
                            .expect("committed queue")
                            .expect("durable before ack");
                        if let Some(id) = request_id {
                            assert_eq!(work.request_id, id);
                        } else {
                            request_id = Some(work.request_id);
                        }
                        break;
                    }
                    Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.expect("pong"),
                    other => panic!("unexpected frame: {other:?}"),
                }
            }
        }
        socket.close(None).await.expect("close");
    });
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
        .await
        .expect("client handshake");
    let receiver = Receiver {
        api: Arc::new(
            SlackApi::new("xoxb-test".to_owned(), "xapp-test".to_owned()).expect("client"),
        ),
        store,
        active: Arc::new(Active::default()),
        wake: Arc::new(Notify::new()),
        shutdown: CancellationToken::new(),
        team: "T1".to_owned(),
        bot: "U2".to_owned(),
        user: "U3".to_owned(),
    };
    tokio::time::timeout(Duration::from_secs(10), receive(&receiver, &mut socket))
        .await
        .expect("bounded receive")
        .expect("receive");
    server.await.expect("server");
}
