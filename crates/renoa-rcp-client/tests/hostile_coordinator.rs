//! The client against a coordinator, or something in front of it, that
//! misbehaves.

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_rcp_client::{ClientError, DeviceKey, ask_to_sign_in};
use renoa_rcp_wire::{ClientMessage, DeviceCredential, DeviceCredentials, DeviceId, ServerMessage};
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _},
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use uuid::Uuid;

/// A software key that counts what it signs.
struct CountingKey {
    pair: EcdsaKeyPair,
    signed: AtomicUsize,
}

impl CountingKey {
    fn generate() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
            .expect("generate key");
        Self {
            pair: EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                .expect("load key"),
            signed: AtomicUsize::new(0),
        }
    }
}

impl DeviceKey for CountingKey {
    fn public_key(&self) -> [u8; 65] {
        self.pair
            .public_key()
            .as_ref()
            .try_into()
            .expect("uncompressed point")
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, ClientError> {
        self.signed.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .pair
            .sign(&SystemRandom::new(), message)
            .expect("sign")
            .as_ref()
            .to_vec())
    }
}

fn credentials() -> DeviceCredentials {
    DeviceCredentials {
        device_id: DeviceId::new(),
        credential: DeviceCredential::new("credential"),
    }
}

/// A coordinator that answers every proof with another challenge, and counts
/// the proofs it receives until the client goes.
async fn challenging_forever() -> (String, JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("ws://{}/connect", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut socket = accept_async(stream).await.expect("websocket");
        let mut proofs = 0;
        let mut frame = socket.next().await;
        while let Some(Ok(Message::Text(text))) = frame {
            match serde_json::from_str::<ClientMessage>(&text).expect("client frame") {
                ClientMessage::Authenticate { .. } => {}
                ClientMessage::DeviceProof { .. } => proofs += 1,
                other => panic!("unexpected frame {other:?}"),
            }
            let challenge = ServerMessage::DeviceChallenge {
                nonce: format!("nonce-{proofs}"),
            };
            let json = serde_json::to_string(&challenge).expect("encode");
            if socket.send(Message::Text(json.into())).await.is_err() {
                break;
            }
            frame = socket.next().await;
        }
        proofs
    });
    (url, server)
}

#[tokio::test]
async fn a_key_answers_one_challenge_and_a_second_ends_the_connection() {
    let (url, server) = challenging_forever().await;
    let key = CountingKey::generate();
    let refused = timeout(
        Duration::from_secs(10),
        renoa_rcp_client::connect_with_key(&url, credentials(), &key),
    )
    .await
    .expect("the client stops answering");
    assert!(matches!(refused, Err(ClientError::Protocol(_))));
    assert_eq!(key.signed.load(Ordering::SeqCst), 1);
    assert_eq!(server.await.expect("server"), 1);
}

#[tokio::test]
async fn a_connection_without_a_key_refuses_a_challenge_and_says_why() {
    let (url, server) = challenging_forever().await;
    let Err(ClientError::Protocol(reason)) = renoa_rcp_client::connect(&url, credentials()).await
    else {
        panic!("a keyless connection cannot answer a challenge");
    };
    assert!(reason.contains("registered a key"), "{reason}");
    assert_eq!(server.await.expect("server"), 0);
}

/// An HTTP server that gives every request `answer` and counts the requests.
async fn answering(answer: String) -> (String, JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        let mut requests = 0;
        while let Ok(Ok((mut stream, _))) =
            timeout(Duration::from_millis(500), listener.accept()).await
        {
            requests += 1;
            let mut request = vec![0; 4096];
            let _ = stream.read(&mut request).await;
            stream.write_all(answer.as_bytes()).await.expect("answer");
        }
        requests
    });
    (origin, server)
}

fn http(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n{headers}\r\n{body}",
        body.len()
    )
}

async fn ask(origin: &str) -> Result<renoa_rcp_client::SignInRequest, ClientError> {
    ask_to_sign_in(
        origin,
        Uuid::new_v4(),
        "mac",
        "MacBook",
        &CountingKey::generate(),
    )
    .await
}

#[tokio::test]
async fn a_proxy_that_cannot_reach_renoa_is_a_transport_failure() {
    let (origin, _server) = answering(http(
        "502 Bad Gateway",
        "content-type: text/html\r\n",
        "<h1>502</h1>",
    ))
    .await;
    assert!(matches!(ask(&origin).await, Err(ClientError::Transport(_))));
}

#[tokio::test]
async fn a_redirect_is_not_followed_with_the_proof() {
    let (elsewhere, followed) = answering(http("200 OK", "", "{}")).await;
    let (origin, _server) = answering(http(
        "307 Temporary Redirect",
        &format!("location: {elsewhere}/v1/identity/device-requests\r\n"),
        "",
    ))
    .await;
    assert!(matches!(ask(&origin).await, Err(ClientError::Protocol(_))));
    assert_eq!(followed.await.expect("server"), 0);
}

#[tokio::test]
async fn an_answer_may_carry_fields_this_client_does_not_know() {
    let (origin, _server) = answering(http(
        "200 OK",
        "content-type: application/json\r\n",
        r#"{"code":"7K3MQX9D","expiresAtMs":1,"addedLater":true}"#,
    ))
    .await;
    let asked = ask(&origin).await.expect("the answer is read");
    assert_eq!((asked.code.expose(), asked.expires_at_ms), ("7K3MQX9D", 1));
}
