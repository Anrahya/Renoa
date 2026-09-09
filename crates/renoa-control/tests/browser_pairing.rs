use std::{
    path::Path,
    time::{Duration, SystemTime},
};

use renoa_control::{BrowserSessions, Coordinator};
use renoa_protocol::PrincipalId;
use reqwest::{Client, Response, StatusCode};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost";

struct Server {
    url: String,
    stop: CancellationToken,
    task: JoinHandle<Result<(), renoa_control::ControlError>>,
}

impl Server {
    async fn start(path: &Path) -> Self {
        let coordinator =
            Coordinator::open_with_passkeys(path, "localhost", ORIGIN).expect("identity service");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let stop = CancellationToken::new();
        let task = tokio::spawn(coordinator.serve(listener, stop.clone()));
        Self { url, stop, task }
    }
    async fn close(self) {
        self.stop.cancel();
        self.task.await.expect("join server").expect("stop server");
    }
    async fn pair(&self, client: &Client, body: &Value) -> Response {
        client
            .post(format!("{}/v1/identity/pair", self.url))
            .header("origin", ORIGIN)
            .json(body)
            .send()
            .await
            .expect("pair request")
    }
}

#[tokio::test]
async fn pairing_retries_after_restart_but_cannot_switch_browser_or_revive_logout() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("identity.sqlite");
    let server = Server::start(&path).await;
    let sessions = BrowserSessions::open(&path).expect("sessions");
    let owner = PrincipalId::from_uuid(Uuid::new_v4());
    let token = sessions
        .create_pairing(owner, SystemTime::now() + Duration::from_mins(30))
        .await
        .expect("pairing code");
    let body = json!({"pairingToken":token,"browserNonce":"12".repeat(32)});
    let client = Client::new();
    for origin in [None, Some("https://attacker.invalid")] {
        let mut request = client
            .post(format!("{}/v1/identity/pair", server.url))
            .json(&body);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        assert_eq!(
            request.send().await.expect("forged request").status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let first = server.pair(&client, &body).await;
    assert_eq!(first.status(), StatusCode::OK);
    let cookie = first.headers()["set-cookie"]
        .to_str()
        .expect("cookie")
        .to_owned();
    for flag in ["Secure", "HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(cookie.contains(flag));
    }
    let identity: Value = first.json().await.expect("identity");
    assert_eq!(identity, json!({"principalId":owner}));
    let pair = cookie.split(';').next().expect("cookie pair");
    server.close().await;
    let server = Server::start(&path).await;
    // Simulate losing the first HTTP response after admission and then retrying.
    let retry = server.pair(&client, &body).await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(
        retry.headers()["set-cookie"]
            .to_str()
            .expect("cookie")
            .split(';')
            .next(),
        Some(pair)
    );
    let db = rusqlite::Connection::open(&path).expect("storage inspection");
    let (count, digest): (i64, Vec<u8>) = db
        .query_row(
            "SELECT count(*),token_hash FROM browser_sessions",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("one session");
    assert_eq!(count, 1);
    assert_eq!(digest.len(), 32);
    let mut other = body.clone();
    other["browserNonce"] = json!("34".repeat(32));
    assert_eq!(
        server.pair(&client, &other).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let identity: Value = client
        .get(format!("{}/v1/identity/session", server.url))
        .header("cookie", pair)
        .send()
        .await
        .expect("remembered session")
        .error_for_status()
        .expect("session OK")
        .json()
        .await
        .expect("JSON");
    assert_eq!(identity["principalId"], owner.to_string());
    let ticket = client
        .post(format!("{}/v1/identity/connection-ticket", server.url))
        .header("cookie", pair)
        .header("origin", ORIGIN)
        .json(&json!({"surface":"control_room"}))
        .send()
        .await
        .expect("transport ticket");
    assert_eq!(ticket.status(), StatusCode::OK);
    assert_eq!(
        client
            .post(format!("{}/v1/identity/logout", server.url))
            .header("cookie", pair)
            .header("origin", ORIGIN)
            .send()
            .await
            .expect("logout")
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        server.pair(&client, &body).await.status(),
        StatusCode::UNAUTHORIZED
    );
    server.close().await;
}

#[tokio::test]
async fn only_one_browser_can_claim_a_code_and_revocation_covers_pairings() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("identity.sqlite");
    let server = Server::start(&path).await;
    let sessions = BrowserSessions::open(&path).expect("sessions");
    let owner = PrincipalId::from_uuid(Uuid::new_v4());
    let token = sessions
        .create_pairing(owner, SystemTime::now() + Duration::from_mins(30))
        .await
        .expect("code");
    let client = Client::new();
    let first = json!({"pairingToken":token,"browserNonce":"11".repeat(32)});
    let second = json!({"pairingToken":token,"browserNonce":"22".repeat(32)});
    let (a, b) = tokio::join!(server.pair(&client, &first), server.pair(&client, &second));
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 401]);
    let winning = if a.status() == StatusCode::OK {
        &first
    } else {
        &second
    };
    sessions.revoke(owner).await.expect("revoke owner logins");
    assert_eq!(
        server.pair(&client, winning).await.status(),
        StatusCode::UNAUTHORIZED
    );
    server.close().await;
}

#[tokio::test]
async fn expired_or_wrong_authority_codes_cannot_pair_and_failures_do_not_consume_grants() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("identity.sqlite");
    let server = Server::start(&path).await;
    let sessions = BrowserSessions::open(&path).expect("sessions");
    let owner = PrincipalId::from_uuid(Uuid::new_v4());
    let expiry = SystemTime::now() + Duration::from_mins(30);
    assert!(
        sessions
            .create_pairing(owner, SystemTime::UNIX_EPOCH)
            .await
            .is_err()
    );
    let token = sessions.create_pairing(owner, expiry).await.expect("code");
    let client = Client::new();
    let body = json!({"pairingToken":token,"browserNonce":"56".repeat(32)});
    let mut invalid = body.clone();
    invalid["browserNonce"] = json!("short");
    assert_eq!(
        server.pair(&client, &invalid).await.status(),
        StatusCode::UNAUTHORIZED
    );
    invalid = body.clone();
    invalid["principalId"] = json!(Uuid::new_v4());
    assert_eq!(
        server.pair(&client, &invalid).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let passkey = Coordinator::open(&path)
        .expect("local admin")
        .create_passkey_bootstrap(owner, expiry)
        .await
        .expect("passkey bootstrap");
    invalid = body.clone();
    invalid["pairingToken"] = json!(passkey);
    assert_eq!(
        server.pair(&client, &invalid).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let db = rusqlite::Connection::open(&path).expect("fault injection");
    db.execute_batch("CREATE TRIGGER fail_pairing BEFORE INSERT ON browser_sessions BEGIN SELECT RAISE(ABORT,'unavailable'); END;").expect("fail admission");
    assert_eq!(
        server.pair(&client, &body).await.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    db.execute_batch("DROP TRIGGER fail_pairing;")
        .expect("recover storage");
    assert_eq!(server.pair(&client, &body).await.status(), StatusCode::OK);
    db.execute("UPDATE browser_pairings SET expires_at_ms=0", [])
        .expect("expire code");
    assert_eq!(
        server.pair(&client, &body).await.status(),
        StatusCode::UNAUTHORIZED
    );
    server.close().await;
}

#[tokio::test]
async fn revocation_invalidates_outstanding_codes_for_only_that_owner_across_restart() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("identity.sqlite");
    let server = Server::start(&path).await;
    let sessions = BrowserSessions::open(&path).expect("sessions");
    let owner = PrincipalId::from_uuid(Uuid::new_v4());
    let other = PrincipalId::from_uuid(Uuid::new_v4());
    let expiry = SystemTime::now() + Duration::from_mins(30);
    let client = Client::new();
    let mut browsers = Vec::new();
    for principal in [owner, other] {
        let claimed = sessions
            .create_pairing(principal, expiry)
            .await
            .expect("claimed grant");
        let pending = sessions
            .create_pairing(principal, expiry)
            .await
            .expect("unused grant");
        let claimed = json!({"pairingToken":claimed,"browserNonce":"11".repeat(32)});
        let pending = json!({"pairingToken":pending,"browserNonce":"22".repeat(32)});
        let response = server.pair(&client, &claimed).await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .expect("cookie")
            .split(';')
            .next()
            .expect("cookie pair")
            .to_owned();
        browsers.push((principal, claimed, pending, cookie));
    }
    sessions
        .revoke(owner)
        .await
        .expect("revoke existing authority");
    server.close().await;
    let server = Server::start(&path).await;
    for (principal, claimed, pending, cookie) in browsers {
        let expected = if principal == owner {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::OK
        };
        assert_eq!(
            server.pair(&client, &pending).await.status(),
            expected,
            "unused code for {principal}"
        );
        assert_eq!(
            server.pair(&client, &claimed).await.status(),
            expected,
            "claimed-code replay for {principal}"
        );
        assert_eq!(
            client
                .get(format!("{}/v1/identity/session", server.url))
                .header("cookie", cookie)
                .send()
                .await
                .expect("existing login")
                .status(),
            expected
        );
    }
    sessions.revoke(owner).await.expect("repeated revocation");
    let fresh = sessions
        .create_pairing(owner, expiry)
        .await
        .expect("new recovery grant");
    assert_eq!(
        server
            .pair(
                &client,
                &json!({"pairingToken":fresh,"browserNonce":"33".repeat(32)})
            )
            .await
            .status(),
        StatusCode::OK
    );
    server.close().await;
}

#[tokio::test]
async fn failed_revocation_rolls_back_both_sessions_and_pairing_codes() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("identity.sqlite");
    let server = Server::start(&path).await;
    let sessions = BrowserSessions::open(&path).expect("sessions");
    let owner = PrincipalId::from_uuid(Uuid::new_v4());
    let expiry = SystemTime::now() + Duration::from_mins(30);
    let claimed = sessions
        .create_pairing(owner, expiry)
        .await
        .expect("claimed grant");
    let pending = sessions
        .create_pairing(owner, expiry)
        .await
        .expect("unused grant");
    let claimed = json!({"pairingToken":claimed,"browserNonce":"44".repeat(32)});
    let pending = json!({"pairingToken":pending,"browserNonce":"55".repeat(32)});
    let client = Client::new();
    assert_eq!(
        server.pair(&client, &claimed).await.status(),
        StatusCode::OK
    );
    let db = rusqlite::Connection::open(&path).expect("fault injection");
    db.execute_batch("CREATE TRIGGER fail_revoke BEFORE DELETE ON browser_pairings BEGIN SELECT RAISE(ABORT,'unavailable'); END;")
        .expect("fail second write");
    assert!(sessions.revoke(owner).await.is_err());
    // The existing session and the unused code both survive the failed transaction.
    assert_eq!(
        server.pair(&client, &claimed).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        server.pair(&client, &pending).await.status(),
        StatusCode::OK
    );
    db.execute_batch("DROP TRIGGER fail_revoke;")
        .expect("recover storage");
    sessions.revoke(owner).await.expect("retry revocation");
    for body in [&claimed, &pending] {
        assert_eq!(
            server.pair(&client, body).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    server.close().await;
}
