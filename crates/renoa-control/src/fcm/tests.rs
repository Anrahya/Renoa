use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    extract::State,
    http::{StatusCode, Uri},
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::{Attempt, FcmSender, WakeKind, WakeOutcome, classify};
use crate::PushToken;

/// An `FcmError` of `INVALID_ARGUMENT` beside `BadRequest` violations on
/// `fields`.
fn invalid_argument_with_violations(fields: &[&str]) -> Value {
    let violations: Vec<Value> = fields
        .iter()
        .map(|field| json!({ "field": field, "description": "stub" }))
        .collect();
    json!({
        "error": {
            "status": "INVALID_ARGUMENT",
            "details": [
                {
                    "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                    "errorCode": "INVALID_ARGUMENT",
                },
                {
                    "@type": "type.googleapis.com/google.rpc.BadRequest",
                    "fieldViolations": violations,
                },
            ],
        }
    })
}

#[test]
fn an_invalid_argument_without_a_violation_on_the_token_is_refused_and_keeps_it() {
    let other = "INVALID_ARGUMENT without a violation on the token";
    for (fields, expected) in [
        (&["message.android.ttl"][..], refused(other)),
        (&["message.data", "message.android"][..], refused(other)),
        (&["message.token"][..], dead("INVALID_ARGUMENT")),
        (
            &["message.data", "message.token"][..],
            dead("INVALID_ARGUMENT"),
        ),
        (&[][..], refused(other)),
    ] {
        assert_eq!(
            classified(400, &invalid_argument_with_violations(fields)),
            expected,
            "{fields:?}"
        );
    }
}

fn fcm_error(code: &str, canonical: &str) -> Value {
    json!({
        "error": {
            "status": canonical,
            "details": [{
                "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                "errorCode": code,
            }],
        }
    })
}

fn bad_request(field: &str) -> Value {
    json!({
        "error": {
            "status": "INVALID_ARGUMENT",
            "details": [{
                "@type": "type.googleapis.com/google.rpc.BadRequest",
                "fieldViolations": [{ "field": field, "description": "stub" }],
            }],
        }
    })
}

fn classified(status: u16, body: &Value) -> Attempt {
    let status = StatusCode::from_u16(status).expect("status");
    classify(status, body.to_string().as_bytes(), None)
}

const fn dead(reason: &'static str) -> Attempt {
    Attempt::Done(WakeOutcome::DeadToken(reason))
}

const fn refused(reason: &'static str) -> Attempt {
    Attempt::Done(WakeOutcome::Refused(reason))
}

const fn retry(reason: &'static str) -> Attempt {
    Attempt::Retry {
        reason,
        after: None,
    }
}

#[test]
fn only_a_detail_that_names_the_token_calls_it_dead() {
    let bare = json!({"error": {"code": 400, "status": "INVALID_ARGUMENT", "message": "stub"}});
    for (status, body, expected) in [
        (
            404,
            fcm_error("UNREGISTERED", "NOT_FOUND"),
            dead("UNREGISTERED"),
        ),
        (
            400,
            fcm_error("INVALID_ARGUMENT", "INVALID_ARGUMENT"),
            refused("INVALID_ARGUMENT without a violation on the token"),
        ),
        (400, bad_request("message.token"), dead("INVALID_ARGUMENT")),
        (
            400,
            bad_request("message.android.ttl"),
            refused("FCM gave an unexpected answer"),
        ),
        (400, bare, refused("FCM gave an unexpected answer")),
        (
            400,
            json!("not an error body"),
            refused("FCM gave an unexpected answer"),
        ),
    ] {
        assert_eq!(classified(status, &body), expected, "{status} {body}");
    }
}

#[test]
fn a_configuration_fault_is_refused_and_an_overload_or_gateway_failure_is_retried() {
    let empty = json!({});
    for (status, body, expected) in [
        (
            403,
            fcm_error("SENDER_ID_MISMATCH", "PERMISSION_DENIED"),
            refused("SENDER_ID_MISMATCH"),
        ),
        (
            401,
            fcm_error("THIRD_PARTY_AUTH_ERROR", "UNAUTHENTICATED"),
            refused("THIRD_PARTY_AUTH_ERROR"),
        ),
        (401, empty.clone(), Attempt::Unauthorized),
        (403, empty.clone(), refused("FCM gave an unexpected answer")),
        (
            429,
            fcm_error("QUOTA_EXCEEDED", "RESOURCE_EXHAUSTED"),
            retry("QUOTA_EXCEEDED"),
        ),
        (
            429,
            json!({"error": {"status": "RESOURCE_EXHAUSTED"}}),
            retry("QUOTA_EXCEEDED"),
        ),
        (
            503,
            fcm_error("UNAVAILABLE", "UNAVAILABLE"),
            retry("UNAVAILABLE"),
        ),
        (500, fcm_error("INTERNAL", "INTERNAL"), retry("INTERNAL")),
        (429, empty.clone(), retry("QUOTA_EXCEEDED")),
        (500, empty.clone(), retry("INTERNAL")),
        (502, empty.clone(), retry("BAD_GATEWAY")),
        (503, empty.clone(), retry("UNAVAILABLE")),
        (504, empty, retry("GATEWAY_TIMEOUT")),
    ] {
        assert_eq!(classified(status, &body), expected, "{status} {body}");
    }
}

#[test]
fn a_retry_keeps_the_servers_retry_after() {
    let after = Some(Duration::from_secs(7));
    assert_eq!(
        classify(StatusCode::SERVICE_UNAVAILABLE, b"", after),
        Attempt::Retry {
            reason: "UNAVAILABLE",
            after
        }
    );
}

/// A token endpoint that always grants, and an FCM that gives each wake the
/// next of `statuses`; the paths asked, in order.
async fn google(statuses: Vec<u16>) -> (url::Url, Arc<Mutex<Vec<String>>>) {
    struct Stub {
        statuses: Mutex<VecDeque<u16>>,
        paths: Arc<Mutex<Vec<String>>>,
    }
    async fn answer(State(stub): State<Arc<Stub>>, uri: Uri) -> (StatusCode, String) {
        stub.paths
            .lock()
            .expect("paths")
            .push(uri.path().to_owned());
        if uri.path() == "/token" {
            let granted = json!({"access_token": "ya29.unit", "expires_in": 3599});
            return (StatusCode::OK, granted.to_string());
        }
        let status = stub
            .statuses
            .lock()
            .expect("statuses")
            .pop_front()
            .expect("an answer for every wake");
        (
            StatusCode::from_u16(status).expect("status"),
            "{}".to_owned(),
        )
    }
    let paths = Arc::default();
    let stub = Arc::new(Stub {
        statuses: Mutex::new(statuses.into()),
        paths: Arc::clone(&paths),
    });
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("stub listener");
    let origin = url::Url::parse(&format!(
        "http://{}",
        listener.local_addr().expect("address")
    ))
    .expect("stub origin");
    let app = Router::new().fallback(answer).with_state(stub);
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve the stub") });
    (origin, paths)
}

#[tokio::test]
async fn a_refused_access_token_on_the_last_attempt_is_still_retried_with_a_fresh_one() {
    let (origin, paths) = google(vec![503, 503, 401, 200]).await;
    let files = tempfile::tempdir().expect("temporary directory");
    let settings = crate::google_access::tests::push_settings(files.path())
        .with_google_endpoints(origin.join("/token").expect("token endpoint"), origin);
    let sender = FcmSender::new(&settings).expect("build the sender");
    let outcome = sender
        .wake(
            &PushToken::new("fcm:phone"),
            WakeKind::Changed,
            &CancellationToken::new(),
        )
        .await;
    assert_eq!(outcome, WakeOutcome::Delivered);
    let send = "/v1/projects/renoa-test/messages:send";
    assert_eq!(
        *paths.lock().expect("paths"),
        ["/token", send, send, send, "/token", send]
    );
}
