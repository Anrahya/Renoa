use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};

use super::{Attempt, WakeOutcome, classify};

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
fn an_invalid_argument_on_another_field_is_refused_and_keeps_the_token() {
    let other = "FCM rejected a field other than the token";
    for (fields, expected) in [
        (&["message.android.ttl"][..], refused(other)),
        (&["message.data", "message.android"][..], refused(other)),
        (&["message.token"][..], dead("INVALID_ARGUMENT")),
        (
            &["message.data", "message.token"][..],
            dead("INVALID_ARGUMENT"),
        ),
        (&[][..], dead("INVALID_ARGUMENT")),
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
            dead("INVALID_ARGUMENT"),
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
