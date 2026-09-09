use std::{
    path::Path,
    time::{Duration, SystemTime},
};

use axum::http::{HeaderMap, HeaderValue, header};
use renoa_control::{BrowserSessions, Coordinator};
use renoa_protocol::PrincipalId;
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};

const ORIGIN: &str = "http://localhost";

#[tokio::test]
async fn schema_ten_migration_preserves_existing_passkey_logins() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("identity.sqlite");
    let principal = PrincipalId::from_uuid(Uuid::new_v4());
    let (client, server, cookie, verification) = register_browser(&path, principal).await;
    server.stop().await;
    let db = rusqlite::Connection::open(&path).expect("legacy fixture");
    db.execute_batch("BEGIN;
        ALTER TABLE browser_sessions RENAME TO current_sessions;
        CREATE TABLE browser_sessions (
            token_hash BLOB PRIMARY KEY CHECK(length(token_hash)=32),
            credential_id BLOB NOT NULL REFERENCES passkeys(credential_id) ON DELETE CASCADE,
            expires_at_ms INTEGER NOT NULL
        );
        INSERT INTO browser_sessions SELECT token_hash,credential_id,expires_at_ms FROM current_sessions;
        DROP TABLE current_sessions;
        DROP TABLE browser_pairings;
        PRAGMA user_version=10;
        COMMIT;").expect("schema 10 fixture");
    let server = Server::start(&path).await;
    assert_restored_browser(&client, &server, &cookie, principal, verification).await;
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .expect("version"),
        11
    );
    server.stop().await;
}

struct Server {
    url: String,
    cancel: CancellationToken,
    task: JoinHandle<Result<(), renoa_control::ControlError>>,
}

impl Server {
    async fn start(path: &Path) -> Self {
        let coordinator =
            Coordinator::open_with_passkeys(path, "localhost", ORIGIN).expect("identity server");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let cancel = CancellationToken::new();
        let task = tokio::spawn(coordinator.serve(listener, cancel.clone()));
        Self { url, cancel, task }
    }
    async fn stop(self) {
        self.cancel.cancel();
        self.task
            .await
            .expect("server task")
            .expect("server shutdown");
    }
}

#[tokio::test]
async fn remembered_browser_survives_restart_and_renews_tickets_without_passkey() {
    let files = tempfile::tempdir().expect("test directory");
    let path = files.path().join("identity.sqlite");
    let principal = PrincipalId::from_uuid(Uuid::new_v4());
    let (client, server, pair, verification) = register_browser(&path, principal).await;
    server.stop().await;
    let server = Server::start(&path).await;
    assert_restored_browser(&client, &server, &pair, principal, verification).await;
    assert_session_lifecycle(&path, &client, &server, &pair, principal).await;
    server.stop().await;
}

async fn register_browser(path: &Path, principal: PrincipalId) -> (Client, Server, String, Value) {
    let control = Coordinator::open(path).expect("create identity database");
    let bootstrap = control
        .create_passkey_bootstrap(principal, SystemTime::now() + Duration::from_mins(5))
        .await
        .expect("bootstrap");
    let server = Server::start(path).await;
    let client = Client::new();
    let options: Value = client
        .post(format!(
            "{}/v1/identity/passkeys/registration/options",
            server.url
        ))
        .header("origin", ORIGIN)
        .json(&json!({"bootstrapToken":bootstrap,"surface":"control_room"}))
        .send()
        .await
        .expect("options request")
        .error_for_status()
        .expect("registration options")
        .json()
        .await
        .expect("options JSON");
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = authenticator
        .do_registration(
            url::Url::parse(ORIGIN).expect("origin"),
            serde_json::from_value(options["options"].clone()).expect("creation options"),
        )
        .expect("register passkey");
    let verified = client
        .post(format!(
            "{}/v1/identity/passkeys/registration/verify",
            server.url
        ))
        .header("origin", ORIGIN)
        .json(&json!({"ceremonyId":options["ceremonyId"],"credential":credential}))
        .send()
        .await
        .expect("verify request")
        .error_for_status()
        .expect("registration verified");
    let cookie = verified.headers()["set-cookie"]
        .to_str()
        .expect("cookie string")
        .to_owned();
    for flag in [
        "Secure",
        "HttpOnly",
        "SameSite=Strict",
        "Path=/",
        "Max-Age=",
    ] {
        assert!(cookie.contains(flag));
    }
    assert!(!cookie.contains("Domain="));
    let pair = cookie.split(';').next().expect("cookie pair").to_owned();
    let verification: Value = verified.json().await.expect("verification JSON");
    assert!(
        !verification
            .to_string()
            .contains(pair.split_once('=').expect("token").1)
    );
    let stored: Vec<u8> = rusqlite::Connection::open(path)
        .expect("inspect storage")
        .query_row("SELECT token_hash FROM browser_sessions", [], |r| r.get(0))
        .expect("session digest");
    assert_eq!(stored.len(), 32);
    assert_ne!(stored, pair.as_bytes());
    (client, server, pair, verification)
}

async fn assert_restored_browser(
    client: &Client,
    server: &Server,
    pair: &str,
    principal: PrincipalId,
    verification: Value,
) {
    let identity: Value = client
        .get(format!("{}/v1/identity/session", server.url))
        .header("cookie", pair)
        .send()
        .await
        .expect("restore session")
        .error_for_status()
        .expect("remembered login")
        .json()
        .await
        .expect("identity JSON");
    assert_eq!(identity["principalId"], principal.to_string());
    for surface in ["control_room", "second_browser_surface"] {
        let ticket: Value = client
            .post(format!("{}/v1/identity/connection-ticket", server.url))
            .header("cookie", pair)
            .header("origin", ORIGIN)
            .json(&json!({"surface":surface}))
            .send()
            .await
            .expect("renew ticket")
            .error_for_status()
            .expect("no passkey ceremony")
            .json()
            .await
            .expect("ticket JSON");
        assert_ne!(ticket["connectionTicket"], verification["connectionTicket"]);
    }
    for origin in [None, Some("https://attacker.invalid")] {
        let mut request = client
            .post(format!("{}/v1/identity/logout", server.url))
            .header("cookie", pair);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        assert_eq!(
            request.send().await.expect("reject forged logout").status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        client
            .post(format!("{}/v1/identity/connection-ticket", server.url))
            .header("cookie", pair)
            .header("origin", "https://attacker.invalid")
            .json(&json!({"surface":"control_room"}))
            .send()
            .await
            .expect("reject forged ticket request")
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

async fn assert_session_lifecycle(
    path: &Path,
    client: &Client,
    server: &Server,
    pair: &str,
    principal: PrincipalId,
) {
    let sessions = BrowserSessions::open(path).expect("independent identity consumer");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(pair).expect("cookie header"),
    );
    let now = SystemTime::now();
    let later = now + Duration::from_hours(100 * 24);
    let renewed = sessions
        .authenticate(&headers, later)
        .await
        .expect("renew remembered login")
        .expect("valid session");
    assert_eq!(renewed.principal_id(), principal);
    assert!(
        renewed
            .cookie(later)
            .expect("renewal cookie")
            .to_str()
            .expect("cookie")
            .contains("Max-Age=15552000")
    );
    assert!(
        sessions
            .authenticate(&headers, later + Duration::from_hours(181 * 24))
            .await
            .expect("check expiry")
            .is_none()
    );

    let logged_out = client
        .post(format!("{}/v1/identity/logout", server.url))
        .header("cookie", pair)
        .header("origin", ORIGIN)
        .send()
        .await
        .expect("logout");
    assert_eq!(logged_out.status(), StatusCode::OK);
    assert!(
        logged_out.headers()["set-cookie"]
            .to_str()
            .expect("clear cookie")
            .contains("Max-Age=0")
    );
    assert!(
        sessions
            .authenticate(&headers, now)
            .await
            .expect("recheck logout")
            .is_none()
    );
}

#[tokio::test]
async fn invalid_credentials_and_storage_failure_are_distinct() {
    let files = tempfile::tempdir().expect("directory");
    let path = files.path().join("identity.sqlite");
    assert!(BrowserSessions::open(&path).is_err());
    assert!(!path.exists());
    Coordinator::open(&path).expect("initialize");
    let sessions = BrowserSessions::open(&path).expect("session reader");
    for cookie in ["", "__Host-renoa_session=invalid", "unrelated=cookie"] {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(cookie).expect("header"),
        );
        assert!(
            sessions
                .authenticate(&headers, SystemTime::now())
                .await
                .expect("invalid credential")
                .is_none()
        );
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("__Host-renoa_session={}", "a".repeat(64)))
            .expect("token header"),
    );
    rusqlite::Connection::open(path)
        .expect("fault injection")
        .execute("DROP TABLE browser_sessions", [])
        .expect("break storage");
    assert!(
        sessions
            .authenticate(&headers, SystemTime::now())
            .await
            .is_err()
    );
}
