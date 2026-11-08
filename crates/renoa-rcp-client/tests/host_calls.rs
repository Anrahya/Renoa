//! `call_host` against a coordinator that answers as its relay does. The
//! relay itself is tested in `renoa-control`.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_rcp_client::{ClientError, HostAnswer};
use renoa_rcp_wire::{
    ClientMessage, DeviceCredential, DeviceCredentials, DeviceId, ErrorCode, HostMethod,
    JSON_WS_VERSION, ServerMessage,
};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};

/// A coordinator that signs the client in and answers each Host call with
/// `answer`, reporting every call it receives.
async fn relaying(
    answer: fn(u64, &str) -> ServerMessage,
) -> (
    String,
    mpsc::UnboundedReceiver<ClientMessage>,
    JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("ws://{}/connect", listener.local_addr().expect("address"));
    let (calls, received) = mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut socket = accept_async(stream).await.expect("websocket");
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let frame = serde_json::from_str::<ClientMessage>(&text).expect("client frame");
            let reply = match &frame {
                ClientMessage::Authenticate { .. } => ServerMessage::Authenticated {
                    version: JSON_WS_VERSION,
                },
                ClientMessage::CallHost {
                    request_id, path, ..
                } => answer(*request_id, path),
                other => panic!("unexpected frame {other:?}"),
            };
            let _ = calls.send(frame);
            let json = serde_json::to_string(&reply).expect("encode");
            if socket.send(Message::Text(json.into())).await.is_err() {
                return;
            }
        }
    });
    (url, received, server)
}

fn credentials() -> DeviceCredentials {
    DeviceCredentials {
        device_id: DeviceId::new(),
        credential: DeviceCredential::new("credential"),
    }
}

async fn host_call(received: &mut mpsc::UnboundedReceiver<ClientMessage>) -> ClientMessage {
    loop {
        let frame = timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("a frame arrives")
            .expect("the coordinator runs");
        if matches!(frame, ClientMessage::CallHost { .. }) {
            return frame;
        }
    }
}

#[tokio::test]
async fn a_call_goes_as_asked_and_the_hosts_answer_comes_back_whatever_its_status() {
    let (url, mut received, _server) = relaying(|request_id, path| ServerMessage::HostAnswer {
        request_id,
        status: if path.ends_with("/missing") { 404 } else { 201 },
        body: format!(r#"{{"path":"{path}"}}"#),
    })
    .await;
    let (connection, _events) = renoa_rcp_client::connect(&url, credentials())
        .await
        .expect("connect");

    let made = connection
        .call_host(
            HostMethod::Post,
            "/v1/host/agents",
            Some(r#"{"name":"Ada"}"#.to_owned()),
        )
        .await;
    assert_eq!(
        made,
        Ok(HostAnswer {
            status: 201,
            body: r#"{"path":"/v1/host/agents"}"#.to_owned(),
        })
    );
    let ClientMessage::CallHost {
        method, path, body, ..
    } = host_call(&mut received).await
    else {
        unreachable!("host_call returns only Host calls");
    };
    assert_eq!(
        (method, path.as_str(), body.as_deref()),
        (
            HostMethod::Post,
            "/v1/host/agents",
            Some(r#"{"name":"Ada"}"#)
        )
    );

    let missing = connection
        .call_host(HostMethod::Get, "/v1/host/agents/missing", None)
        .await
        .expect("a Host refusal is an answer");
    assert_eq!(missing.status, 404);
    let ClientMessage::CallHost {
        method, path, body, ..
    } = host_call(&mut received).await
    else {
        unreachable!("host_call returns only Host calls");
    };
    assert_eq!(
        (method, path.as_str(), body),
        (HostMethod::Get, "/v1/host/agents/missing", None)
    );
}

#[tokio::test]
async fn a_host_that_cannot_be_reached_is_the_coordinators_refusal() {
    let (url, _received, _server) = relaying(|request_id, _| ServerMessage::Error {
        request_id: Some(request_id),
        code: ErrorCode::NodeOffline,
        message: "Your Host could not be reached.".to_owned(),
    })
    .await;
    let (connection, _events) = renoa_rcp_client::connect(&url, credentials())
        .await
        .expect("connect");
    let unreachable = connection
        .call_host(HostMethod::Get, "/v1/host/feed", None)
        .await;
    assert!(
        matches!(
            unreachable,
            Err(ClientError::Rejected {
                code: ErrorCode::NodeOffline,
                ..
            })
        ),
        "{unreachable:?}"
    );
}
