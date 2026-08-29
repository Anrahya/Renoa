use std::{fs, time::Duration};

use renoa_kernel::{CommandId, SessionId};
use tokio_util::sync::CancellationToken;

use super::{
    secret::OAuthSecretBundle,
    store::{OAuthFlow, OAuthPhase},
};
use crate::mcp::{
    McpAdapterError, McpConnectionAuth, McpHostError, McpOAuthAuthorizationRequest, McpOAuthError,
};

#[path = "tests/registration.rs"]
mod registration;
mod support;

use support::{CONNECTION, ENDPOINT, Fixture};

#[test]
fn oauth_operation_identity_distinguishes_commands_with_the_same_model_call_id() {
    let session = SessionId::new();
    let first = super::operation_id(session, Some(CommandId::new()), "call-1");
    let second = super::operation_id(session, Some(CommandId::new()), "call-1");
    assert_ne!(first, second);
}

#[tokio::test]
async fn cancelled_browser_authorization_resumes_without_putting_secrets_in_sqlite() {
    let mut fixture = Fixture::new();
    let cancellation = CancellationToken::new();
    let task = {
        let resolver = fixture.resolver.clone();
        let auth = fixture.auth.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            resolver
                .authorize(
                    authorization_request(&auth, "session/tool-call", false),
                    cancellation,
                )
                .await
        })
    };
    wait_for_phase(&fixture, OAuthPhase::AwaitingCallback).await;
    let waiting_bundle = fixture.secret_bundle().await;
    let csrf_state = waiting_bundle.adapter_state["csrf_state"]
        .as_str()
        .expect("stored CSRF state")
        .to_owned();
    cancellation.cancel();
    let result = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("cancelled authorization settles")
        .expect("authorization task joins");
    let Err(error) = result else {
        panic!("cancelled callback must not authorize")
    };
    assert!(matches!(
        error,
        McpHostError::OAuth(McpOAuthError::Cancelled)
    ));

    fixture.enable_callback_browser();
    let authorization = fixture
        .resolver
        .authorize(
            authorization_request(&fixture.auth, "session/tool-call", false),
            CancellationToken::new(),
        )
        .await
        .expect("resume browser authorization");
    assert_eq!(authorization.bearer(), "access-one");
    assert!(
        fixture
            .resolver
            .oauth
            .flows
            .load(CONNECTION)
            .await
            .expect("load completed flow")
            .is_none()
    );

    let database = fs::read(fixture.store.path()).expect("read Host database");
    for secret in ["access-one", "refresh-one", "code-one", csrf_state.as_str()] {
        assert!(
            !database
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()),
            "Host SQLite must not contain {secret}"
        );
    }
    assert_eq!(fixture.action_count("oauth_begin"), 1);
    assert_eq!(fixture.action_count("oauth_exchange"), 1);
}

#[tokio::test]
async fn concurrent_expired_token_reads_perform_one_rotating_refresh() {
    let mut fixture = Fixture::new();
    authorize(&mut fixture).await;
    let mut bundle = fixture.secret_bundle().await;
    bundle
        .adapter_state
        .as_object_mut()
        .expect("OAuth state object")
        .insert("needs_refresh".to_owned(), serde_json::Value::Bool(true));
    fixture.store_bundle(&bundle).await;
    let writes_before = fixture.secret_write_count();

    let first = resolve(&fixture, "session/first");
    let second = resolve(&fixture, "session/second");
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first, "access-two");
    assert_eq!(second, "access-two");
    assert_eq!(fixture.action_count("oauth_refresh"), 1);
    assert_eq!(fixture.secret_write_count(), writes_before + 1);

    let loaded = resolve(&fixture, "session/third").await;
    assert_eq!(loaded, "access-two");
    assert_eq!(fixture.action_count("oauth_refresh"), 1);
    assert_eq!(fixture.secret_write_count(), writes_before + 1);
}

#[tokio::test]
async fn a_lost_refresh_becomes_unknown_and_is_never_replayed() {
    let mut fixture = Fixture::new();
    authorize(&mut fixture).await;
    let mut bundle = fixture.secret_bundle().await;
    bundle
        .adapter_state
        .as_object_mut()
        .expect("OAuth state object")
        .insert("needs_refresh".to_owned(), serde_json::Value::Bool(true));
    bundle
        .adapter_state
        .as_object_mut()
        .expect("OAuth state object")
        .insert("lose_refresh".to_owned(), serde_json::Value::Bool(true));
    fixture.store_bundle(&bundle).await;

    for operation in ["session/first", "session/replayed"] {
        let result = fixture
            .resolver
            .resolve(
                CONNECTION,
                ENDPOINT,
                &fixture.auth,
                operation,
                CancellationToken::new(),
            )
            .await;
        let Err(error) = result else {
            panic!("lost refresh must not produce a credential")
        };
        assert!(matches!(
            error,
            McpHostError::OAuth(McpOAuthError::OutcomeUnknown { .. })
        ));
    }
    assert_eq!(fixture.action_count("oauth_refresh"), 1);
    assert_eq!(
        fixture
            .resolver
            .oauth
            .flows
            .load(CONNECTION)
            .await
            .expect("load unknown flow")
            .expect("unknown flow remains durable")
            .phase,
        OAuthPhase::Unknown
    );
}

#[tokio::test]
async fn a_replayed_restart_uses_its_receipt_instead_of_reauthorizing() {
    let mut fixture = Fixture::new();
    fixture.enable_callback_browser();
    let operation = "session/command/tool-call";
    let first = fixture
        .resolver
        .authorize(
            authorization_request(&fixture.auth, operation, false),
            CancellationToken::new(),
        )
        .await
        .expect("complete initial authorization");
    assert_eq!(first.bearer(), "access-one");

    let replayed = fixture
        .resolver
        .authorize(
            authorization_request(&fixture.auth, operation, true),
            CancellationToken::new(),
        )
        .await
        .expect("replay completed authorization from its receipt");
    assert_eq!(replayed.bearer(), "access-one");
    assert_eq!(fixture.action_count("oauth_begin"), 1);
    assert_eq!(fixture.action_count("oauth_exchange"), 1);
    assert_eq!(fixture.action_count("oauth_token"), 1);

    let next = fixture
        .resolver
        .authorize(
            authorization_request(&fixture.auth, "session/next-command/tool-call", true),
            CancellationToken::new(),
        )
        .await
        .expect("a new operation may explicitly reauthorize");
    assert_eq!(next.bearer(), "access-one");
    assert_eq!(fixture.action_count("oauth_begin"), 2);
    assert_eq!(fixture.action_count("oauth_exchange"), 2);
}

#[tokio::test]
async fn a_definite_exchange_failure_replays_its_receipt_without_a_second_flow() {
    let mut fixture = Fixture::new();
    let cancellation = CancellationToken::new();
    let operation = "session/failed-command/tool-call";
    let task = {
        let resolver = fixture.resolver.clone();
        let auth = fixture.auth.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            resolver
                .authorize(authorization_request(&auth, operation, false), cancellation)
                .await
        })
    };
    wait_for_phase(&fixture, OAuthPhase::AwaitingCallback).await;
    cancellation.cancel();
    let cancelled = task.await.expect("cancelled authorization task joins");
    assert!(
        cancelled.is_err(),
        "cancelled authorization must not complete"
    );

    let mut bundle = fixture.secret_bundle().await;
    bundle
        .adapter_state
        .as_object_mut()
        .expect("OAuth state object")
        .insert("reject_exchange".to_owned(), serde_json::Value::Bool(true));
    fixture.store_bundle(&bundle).await;
    fixture.enable_callback_browser();

    let first = fixture
        .resolver
        .authorize(
            authorization_request(&fixture.auth, operation, false),
            CancellationToken::new(),
        )
        .await;
    let Err(first) = first else {
        panic!("fixture exchange must be rejected")
    };
    assert!(matches!(
        first,
        McpHostError::Adapter(McpAdapterError::Remote(_))
    ));
    assert_eq!(fixture.action_count("oauth_begin"), 1);
    assert_eq!(fixture.action_count("oauth_exchange"), 1);

    let replayed = fixture
        .resolver
        .authorize(
            authorization_request(&fixture.auth, operation, true),
            CancellationToken::new(),
        )
        .await;
    let Err(replayed) = replayed else {
        panic!("the definite exchange failure must be replayed")
    };
    assert!(matches!(
        replayed,
        McpHostError::OAuth(McpOAuthError::ReceiptFailure(_))
    ));
    assert_eq!(fixture.action_count("oauth_begin"), 1);
    assert_eq!(fixture.action_count("oauth_exchange"), 1);
}

#[tokio::test]
async fn begin_recovery_rejects_secret_state_for_a_different_callback() {
    let fixture = Fixture::new();
    let bundle = OAuthSecretBundle::new(serde_json::json!({
        "schema_version": 1,
        "mcp_endpoint": ENDPOINT,
        "csrf_state": "durable-state",
        "redirect_uri": "http://127.0.0.1:41001/oauth/callback",
        "authorization_url": "https://auth.example.test/authorize?state=durable-state&redirect_uri=http%3A%2F%2F127.0.0.1%3A41001%2Foauth%2Fcallback"
    }));
    fixture.store_bundle(&bundle).await;
    fixture
        .resolver
        .oauth
        .flows
        .put(
            &OAuthFlow::interactive(
                CONNECTION,
                "session/interrupted-begin",
                OAuthPhase::BeginInFlight,
                41002,
                i64::MAX,
            )
            .expect("create interrupted flow"),
        )
        .await
        .expect("store interrupted flow");

    let result = fixture
        .resolver
        .authorize(
            authorization_request(&fixture.auth, "session/replay", false),
            CancellationToken::new(),
        )
        .await;
    let Err(error) = result else {
        panic!("a different callback identity must not resume")
    };
    assert!(matches!(
        error,
        McpHostError::OAuth(McpOAuthError::OutcomeUnknown { .. })
    ));
    assert_eq!(fixture.action_count("oauth_begin"), 0);
    assert_eq!(
        fixture
            .resolver
            .oauth
            .flows
            .load(CONNECTION)
            .await
            .expect("load unknown flow")
            .expect("unknown flow remains durable")
            .phase,
        OAuthPhase::Unknown
    );
}

async fn authorize(fixture: &mut Fixture) {
    fixture.enable_callback_browser();
    let authorization = fixture
        .resolver
        .authorize(
            authorization_request(&fixture.auth, "session/authorize", false),
            CancellationToken::new(),
        )
        .await
        .expect("authorize OAuth fixture");
    assert_eq!(authorization.bearer(), "access-one");
}

async fn resolve(fixture: &Fixture, operation: &str) -> String {
    fixture
        .resolver
        .resolve(
            CONNECTION,
            ENDPOINT,
            &fixture.auth,
            operation,
            CancellationToken::new(),
        )
        .await
        .expect("resolve OAuth token")
        .expect("OAuth returns authorization")
        .bearer()
        .to_owned()
}

fn authorization_request<'a>(
    auth: &'a McpConnectionAuth,
    operation_id: &'a str,
    restart: bool,
) -> McpOAuthAuthorizationRequest<'a> {
    McpOAuthAuthorizationRequest {
        connection_id: CONNECTION,
        endpoint: ENDPOINT,
        reference: auth,
        operation_id,
        restart,
        updates: None,
    }
}

async fn wait_for_phase(fixture: &Fixture, expected: OAuthPhase) {
    for _ in 0..200 {
        if fixture
            .resolver
            .oauth
            .flows
            .load(CONNECTION)
            .await
            .expect("load OAuth flow")
            .is_some_and(|flow| flow.phase == expected)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("OAuth flow did not reach {expected:?}");
}
