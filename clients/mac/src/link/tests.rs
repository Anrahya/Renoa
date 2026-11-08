use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use renoa_rcp_wire::{
    ApprovedCredentials, DeviceCode, DeviceCredential, DeviceCredentials, DeviceId,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
    sync::mpsc,
    time::timeout,
};

use super::{Endpoints, Pace, SignInStep, sign_in};
use crate::key::MacKey;

const QUICK: Pace = Pace {
    collect_every: Duration::from_millis(5),
    retry_after: Duration::from_millis(5),
    agents_every: Duration::from_millis(20),
};

/// What the fake Renoa was asked: each call's path and request id.
type Asked = Arc<Mutex<Vec<(String, String)>>>;

/// A Renoa that gives the calls `answers` in order and repeats the last.
async fn renoa(answers: Vec<String>) -> (Endpoints, Asked) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let endpoints = Endpoints::at(&format!(
        "http://{}",
        listener.local_addr().expect("address")
    ));
    let asked = Asked::default();
    let log = Arc::clone(&asked);
    tokio::spawn(async move {
        let mut index = 0;
        while let Ok((mut stream, _)) = listener.accept().await {
            let request = read_request(&mut stream).await;
            let line = request.lines().next().unwrap_or_default().to_owned();
            let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
            let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
            let request_id = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|json| json["requestId"].as_str().map(str::to_owned))
                .unwrap_or_default();
            log.lock().expect("log").push((path, request_id));
            let answer = &answers[index.min(answers.len() - 1)];
            index += 1;
            let _ = stream.write_all(answer.as_bytes()).await;
        }
    });
    (endpoints, asked)
}

/// Reads one request: its head, then as much body as it says it has.
async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        request.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&request).into_owned();
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if request.len() >= end + 4 + length || read == 0 {
                return text;
            }
        } else if read == 0 {
            return text;
        }
    }
}

fn http(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn code(code: &str) -> String {
    http("200 OK", &format!(r#"{{"code":"{code}","expiresAtMs":1}}"#))
}

fn waiting() -> String {
    http("202 Accepted", r#"{"expiresAtMs":1}"#)
}

fn refused(status: &str, code: &str) -> String {
    http(status, &format!(r#"{{"code":"{code}","message":"no"}}"#))
}

fn approved(credentials: &DeviceCredentials) -> String {
    let body = serde_json::to_string(&ApprovedCredentials {
        credentials: credentials.clone(),
    })
    .expect("credentials json");
    http("200 OK", &body)
}

fn credentials() -> DeviceCredentials {
    DeviceCredentials {
        device_id: DeviceId::new(),
        credential: DeviceCredential::new("kept-secret"),
    }
}

/// Runs signing in to its end and returns every step it showed.
async fn steps_of(
    endpoints: Endpoints,
    keep: impl Fn(&DeviceCredentials) -> Result<(), String>,
) -> Vec<SignInStep> {
    let (sender, mut steps) = mpsc::unbounded_channel();
    let key = Arc::new(MacKey::generate().expect("key"));
    timeout(
        Duration::from_secs(10),
        sign_in(endpoints, key, "Mac".to_owned(), QUICK, keep, sender),
    )
    .await
    .expect("signing in ends");
    let mut shown = Vec::new();
    while let Ok(step) = steps.try_recv() {
        shown.push(step);
    }
    shown
}

#[tokio::test]
async fn only_an_expired_request_is_replaced_and_every_other_failure_collects_the_same_one() {
    let approval = credentials();
    let (endpoints, asked) = renoa(vec![
        code("AAAA2222"),
        refused("404 Not Found", "not_found"),
        code("BBBB3333"),
        waiting(),
        http("502 Bad Gateway", "proxy"),
        refused("409 Conflict", "conflict"),
        approved(&approval),
    ])
    .await;
    let kept = Mutex::new(None);
    let steps = steps_of(endpoints, |credentials| {
        *kept.lock().expect("kept") = Some(credentials.clone());
        Ok(())
    })
    .await;

    assert_eq!(
        steps,
        [
            SignInStep::Showing(DeviceCode::new("AAAA2222")),
            SignInStep::Showing(DeviceCode::new("BBBB3333")),
            SignInStep::Reachable(false),
            SignInStep::Reachable(true),
            SignInStep::SignedIn(approval.clone()),
        ]
    );
    assert_eq!(kept.into_inner().expect("kept"), Some(approval));
    let asked = asked.lock().expect("asked").clone();
    let ids: Vec<&str> = asked.iter().map(|(_, id)| id.as_str()).collect();
    assert_eq!(ids.len(), 7);
    assert_eq!(ids[0], ids[1], "the expired request was collected");
    assert_ne!(ids[1], ids[2], "a new code is a new request");
    assert!(
        ids[2..].iter().all(|id| *id == ids[2]),
        "a failed collection collects the same request again: {ids:?}"
    );
    assert_eq!(asked[0].0, "/v1/identity/device-requests");
    assert_eq!(asked[1].0, "/v1/identity/device-requests/collect");
}

#[tokio::test]
async fn a_credential_the_keychain_refuses_does_not_sign_in() {
    let (endpoints, _) = renoa(vec![code("AAAA2222"), approved(&credentials())]).await;
    let steps = steps_of(endpoints, |_| Err("locked".to_owned())).await;
    assert_eq!(
        steps,
        [
            SignInStep::Showing(DeviceCode::new("AAAA2222")),
            SignInStep::NotKept("locked".to_owned()),
        ]
    );
}

#[tokio::test]
async fn signing_in_stops_once_no_view_listens() {
    let (endpoints, asked) = renoa(vec![code("AAAA2222"), waiting()]).await;
    let (sender, mut steps) = mpsc::unbounded_channel();
    let key = Arc::new(MacKey::generate().expect("key"));
    let signing_in = tokio::spawn(sign_in(
        endpoints,
        key,
        "Mac".to_owned(),
        QUICK,
        |_| Ok(()),
        sender,
    ));
    assert!(matches!(steps.recv().await, Some(SignInStep::Showing(_))));
    drop(steps);
    timeout(Duration::from_secs(10), signing_in)
        .await
        .expect("signing in stops")
        .expect("signing in ran");
    let calls = asked.lock().expect("asked").len();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        asked.lock().expect("asked").len(),
        calls,
        "no call after it stopped"
    );
}
