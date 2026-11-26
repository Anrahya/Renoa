//! `call_host` against a coordinator that answers as its relay does. The
//! relay itself is tested in `renoa-control`.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_rcp_client::{ClientError, DeviceKey, HeldNotice, HostAnswer, NoticeKey, ShareLink};
use renoa_rcp_wire::{
    ClientMessage, DeviceCredential, DeviceCredentials, DeviceId, ErrorCode, HostMethod,
    JSON_WS_VERSION, ServerMessage,
};
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _},
};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use uuid::Uuid;

/// A coordinator that signs the client in and answers each request with
/// `answer`, reporting every frame it receives.
async fn serving(
    answer: impl Fn(&ClientMessage) -> ServerMessage + Send + Sync + 'static,
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
                other => answer(other),
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

/// A coordinator that signs the client in and answers each Host call with
/// `answer`, reporting every call it receives.
async fn relaying(
    answer: fn(u64, &str) -> ServerMessage,
) -> (
    String,
    mpsc::UnboundedReceiver<ClientMessage>,
    JoinHandle<()>,
) {
    serving(move |frame| match frame {
        ClientMessage::CallHost {
            request_id, path, ..
        } => answer(*request_id, path),
        other => panic!("unexpected frame {other:?}"),
    })
    .await
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

#[tokio::test]
async fn client_requests_share_link_and_surfaces_coordinator_errors() {
    let wanted = Uuid::new_v4();
    let missing = Uuid::new_v4();
    let (url, mut received, _server) = serving(move |frame| match frame {
        ClientMessage::ShareLink {
            request_id,
            share_id,
        } if *share_id == wanted => ServerMessage::ShareLinkIssued {
            request_id: *request_id,
            path: ShareLink::new(format!("/s/{share_id}?ticket=one-time")),
        },
        ClientMessage::ShareLink { request_id, .. } => ServerMessage::Error {
            request_id: Some(*request_id),
            code: ErrorCode::NotFound,
            message: "share not found".to_owned(),
        },
        other => panic!("unexpected frame {other:?}"),
    })
    .await;
    let (connection, _events) = renoa_rcp_client::connect(&url, credentials())
        .await
        .expect("connect");

    let link = connection
        .share_link(wanted)
        .await
        .expect("issued share link");
    assert_eq!(link.expose(), format!("/s/{wanted}?ticket=one-time"));

    loop {
        let frame = timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("frame")
            .expect("open");
        if let ClientMessage::ShareLink { share_id, .. } = frame {
            assert_eq!(share_id, wanted);
            break;
        }
    }

    let err = connection
        .share_link(missing)
        .await
        .expect_err("missing share rejected");
    assert!(
        matches!(
            err,
            ClientError::Rejected {
                code: ErrorCode::NotFound,
                ..
            }
        ),
        "{err:?}"
    );
}

struct TestKey(EcdsaKeyPair);

impl TestKey {
    fn generate() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
            .expect("generate device key");
        Self(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                .expect("load device key"),
        )
    }
}

impl DeviceKey for TestKey {
    fn public_key(&self) -> [u8; 65] {
        self.0
            .public_key()
            .as_ref()
            .try_into()
            .expect("uncompressed P-256 point")
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, ClientError> {
        self.0
            .sign(&SystemRandom::new(), message)
            .map(|sig| sig.as_ref().to_vec())
            .map_err(|_| ClientError::Key("sign failed".to_owned()))
    }
}

/// Like [`serving`], but issues a device challenge so the client proves its
/// device key via `connect_with_key`.
async fn serving_with_key(
    answer: impl Fn(&ClientMessage) -> ServerMessage + Send + Sync + 'static,
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
                ClientMessage::Authenticate { .. } => ServerMessage::DeviceChallenge {
                    nonce: "challenge-nonce".to_owned(),
                },
                ClientMessage::DeviceProof { .. } => ServerMessage::Authenticated {
                    version: JSON_WS_VERSION,
                },
                other => answer(other),
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

#[tokio::test]
async fn client_registers_notice_key_lists_and_acknowledges_notices() {
    let key = NoticeKey::new(
        "BF-DLVvm3C97gH6MzIEC7j1M0GDuNjRhHpAeTO0yeUTB9wUgMWG2wmkQOontZLJUCELALgwqPvlQuCRxhOTuqoM",
    );
    let expected_key = key.clone();
    let held = HeldNotice {
        notice_id: Uuid::new_v4(),
        sealed: "opaque-ciphertext".to_owned(),
        expires_at_ms: 1_893_456_000_000,
    };
    let held_reply = held.clone();
    let (url, mut received, _server) = serving_with_key(move |frame| match frame {
        ClientMessage::SetNoticeKey {
            request_id,
            public_key,
        } => {
            assert_eq!(*public_key, expected_key);
            ServerMessage::NoticeKeySet {
                request_id: *request_id,
            }
        }
        ClientMessage::ListNotices { request_id } => ServerMessage::NoticeList {
            request_id: *request_id,
            notices: vec![held_reply.clone()],
        },
        ClientMessage::AcknowledgeNotice {
            request_id,
            notice_id,
        } if *notice_id == held_reply.notice_id => ServerMessage::NoticeAcknowledged {
            request_id: *request_id,
        },
        ClientMessage::AcknowledgeNotice { request_id, .. } => ServerMessage::Error {
            request_id: Some(*request_id),
            code: ErrorCode::InvalidRole,
            message: "not an app".to_owned(),
        },
        other => panic!("unexpected frame {other:?}"),
    })
    .await;
    let device_key = TestKey::generate();
    let (connection, _events) =
        renoa_rcp_client::connect_with_key(&url, credentials(), &device_key)
            .await
            .expect("connect with device key");

    connection
        .set_notice_key(key.clone())
        .await
        .expect("set notice key");
    assert_eq!(
        connection.list_notices().await.expect("list notices"),
        vec![held.clone()]
    );
    connection
        .acknowledge_notice(held.notice_id)
        .await
        .expect("acknowledge notice");

    let err = connection
        .acknowledge_notice(Uuid::new_v4())
        .await
        .expect_err("refused ack");
    assert_eq!(err.code(), Some(ErrorCode::InvalidRole));

    let mut seen = Vec::new();
    while let Ok(Some(frame)) = timeout(Duration::from_millis(200), received.recv()).await {
        seen.push(frame);
        if seen.len() == 6 {
            break;
        }
    }
    assert!(
        seen.iter()
            .any(|frame| matches!(frame, ClientMessage::DeviceProof { .. }))
    );
    assert!(seen.iter().any(|frame| matches!(
        frame,
        ClientMessage::SetNoticeKey { public_key, .. } if *public_key == key
    )));
    assert!(
        seen.iter()
            .any(|frame| matches!(frame, ClientMessage::ListNotices { .. }))
    );
    assert!(seen.iter().any(|frame| matches!(
        frame,
        ClientMessage::AcknowledgeNotice { notice_id, .. } if *notice_id == held.notice_id
    )));
}
