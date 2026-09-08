use super::*;
use axum::{
    Router,
    extract::{Path as RoutePath, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use renoa_local::{
    GitHubReviewWebhook, LocalHostAdapters, LocalModelConfiguration, ModelProvider, arcee_profile,
};
use ring::hmac;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

fn host(root: &Path) -> LocalHost {
    LocalHost::new(
        root,
        LocalModelConfiguration::new(
            root.join("bridge.mjs"),
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "fixture",
            root.join("auth.sqlite"),
        ),
        vec![arcee_profile(root).expect("profile")],
        LocalHostAdapters::new(None),
    )
    .expect("host")
}

struct Fixture {
    host: LocalHost,
    origin: Url,
    ids: [Uuid; 3],
    posts: Mutex<Vec<u64>>,
    failure: Mutex<Option<StatusCode>>,
}

async fn admit(host: &LocalHost, id: Uuid) {
    use std::fmt::Write as _;
    let secret = b"recovery receipt fixture";
    let body = b"{}";
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, secret), body);
    let mut signature = "sha256=".to_owned();
    for b in tag.as_ref() {
        write!(signature, "{b:02x}").expect("hex");
    }
    host.admit_github_review_webhook(
        GitHubReviewWebhook {
            delivery_id: id,
            event: "ping",
            signature: &signature,
            body,
        },
        secret,
        100,
        CancellationToken::new(),
    )
    .await
    .expect("durable receipt");
}

async fn deliveries(
    State(state): State<Arc<Fixture>>,
    Query(query): Query<HashMap<String, String>>,
) -> (HeaderMap, String) {
    let mut headers = HeaderMap::new();
    let ids: &[u64] = if query.contains_key("cursor") {
        &[2, 3]
    } else {
        headers.insert(
            "link",
            format!(
                "<{}app/hook/deliveries?per_page=100&cursor=next>; rel=\"next\"",
                state.origin
            )
            .parse()
            .expect("link"),
        );
        &[1, 2]
    };
    let body: Vec<_> = ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "id":id,"guid":state.ids[usize::try_from(id-1).expect("index")],
                "event":"pull_request","status_code":if *id==3 {0} else {503},
                "delivered_at":"2026-09-08T00:00:00Z",
            })
        })
        .collect();
    (headers, serde_json::to_string(&body).expect("json"))
}

async fn redeliver(State(state): State<Arc<Fixture>>, RoutePath(id): RoutePath<u64>) -> StatusCode {
    state.posts.lock().expect("posts").push(id);
    if id == 2
        && let Some(status) = *state.failure.lock().expect("failure")
    {
        return status;
    }
    // GitHub redelivery preserves the GUID. The HTTP signature/admission path
    // is covered separately; this fixture commits its resulting Host receipt.
    admit(
        &state.host,
        state.ids[usize::try_from(id - 1).expect("index")],
    )
    .await;
    StatusCode::ACCEPTED
}

#[tokio::test]
async fn missed_deliveries_are_recovered_across_pages_without_repeating_admitted_work() {
    let root = tempfile::tempdir().expect("root");
    let first = host(root.path());
    let ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    admit(&first, ids[0]).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let origin: Url = format!("http://{}/", listener.local_addr().expect("address"))
        .parse()
        .expect("url");
    let state = Arc::new(Fixture {
        host: first.clone(),
        origin: origin.clone(),
        ids,
        posts: Mutex::new(Vec::new()),
        failure: Mutex::new(Some(StatusCode::NOT_FOUND)),
    });
    let router = Router::new()
        .route("/app/hook/deliveries", get(deliveries))
        .route("/app/hook/deliveries/{id}/attempts", post(redeliver))
        .with_state(state.clone());
    let stop = CancellationToken::new();
    let server_stop = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_stop.cancelled_owned())
            .await
            .expect("server");
    });
    let now = "2026-09-08T01:00:00Z"
        .parse::<jiff::Timestamp>()
        .expect("time")
        .as_millisecond();
    let api = Api::new(origin, "fixture-jwt").expect("client");
    api.scan(&first, now, &stop).await.expect("recovery");
    assert_eq!(*state.posts.lock().expect("posts"), vec![2, 3]);
    assert!(!first.has_github_delivery(ids[1]).await.expect("receipt"));
    assert!(first.has_github_delivery(ids[2]).await.expect("receipt"));
    // A permanent failure was skipped, not acknowledged. A subsequent scan
    // can recover that delivery once GitHub accepts it again.
    *state.failure.lock().expect("failure") = None;
    drop(first);
    api.scan(&host(root.path()), now, &stop)
        .await
        .expect("restart recovery");
    assert_eq!(*state.posts.lock().expect("posts"), vec![2, 3, 2]);
    api.scan(&host(root.path()), now, &stop)
        .await
        .expect("dedup");
    assert_eq!(*state.posts.lock().expect("posts"), vec![2, 3, 2]);

    // A separate Host has no receipts. Global failures must prevent later
    // redeliveries, even though an individual 404 above did not.
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let fresh = tempfile::tempdir().expect("fresh Host");
        *state.failure.lock().expect("failure") = Some(status);
        state.posts.lock().expect("posts").clear();
        let error = api
            .scan(&host(fresh.path()), now, &stop)
            .await
            .expect_err("global failure");
        assert!(
            matches!(error, GitHubReviewError::Api { status: code, .. } if code == status.as_u16())
        );
        assert_eq!(*state.posts.lock().expect("posts"), vec![1, 2]);
    }
    stop.cancel();
    server.await.expect("joined");
}

#[tokio::test]
async fn api_backoff_survives_restart_and_pagination_cannot_exfiltrate_app_authentication() {
    let root = tempfile::tempdir().expect("root");
    let error = GitHubReviewError::Api {
        status: 429,
        retry_after: Some("3600".to_owned()),
    };
    let next = retry_at(&error, 1000);
    assert_eq!(next, 3_601_000);
    let path = root.path().join("next.json");
    save_next(&path, next)
        .await
        .expect("persist before network");
    assert_eq!(
        serde_json::from_slice::<i64>(&tokio::fs::read(path).await.expect("restart read"))
            .expect("state"),
        next
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "link",
        "<https://attacker.test/app/hook/deliveries?cursor=1>; rel=\"next\""
            .parse()
            .expect("header"),
    );
    assert!(
        next_page(
            &headers,
            &Url::parse("https://api.github.com").expect("origin")
        )
        .is_err()
    );
}
