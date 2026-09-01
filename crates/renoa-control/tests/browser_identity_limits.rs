use std::time::{Duration, SystemTime};

use renoa_control::Coordinator;
use renoa_protocol::PrincipalId;
use reqwest::StatusCode;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationOptionsRequest<'a> {
    bootstrap_token: &'a renoa_control::PasskeyBootstrapToken,
    surface: &'a str,
}

#[test]
fn passkey_configuration_requires_one_exact_secure_origin() {
    let files = tempfile::tempdir().expect("temporary directory");
    let database = files.path().join("control.sqlite");
    assert!(Coordinator::open_with_passkeys(&database, "renoa.live", "http://renoa.live").is_err());
    assert!(
        Coordinator::open_with_passkeys(&database, "different.example", "https://renoa.live")
            .is_err()
    );
    assert!(
        Coordinator::open_with_passkeys(&database, "renoa.live", "https://renoa.live/path")
            .is_err()
    );
}

#[tokio::test]
async fn active_ceremonies_are_bounded_without_consuming_the_rejected_bootstrap() {
    let files = tempfile::tempdir().expect("temporary directory");
    let database = files.path().join("control.sqlite");
    let principal_id = PrincipalId::from_uuid(Uuid::from_u128(1));
    let coordinator = Coordinator::open(&database).expect("open coordinator");
    let mut bootstraps = Vec::new();
    for _ in 0..65 {
        bootstraps.push(
            coordinator
                .create_passkey_bootstrap(principal_id, SystemTime::now() + Duration::from_mins(5))
                .await
                .expect("create bootstrap"),
        );
    }
    drop(coordinator);

    let coordinator = Coordinator::open_with_passkeys(&database, "localhost", "http://localhost")
        .expect("open passkey coordinator");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind coordinator");
    let address = listener.local_addr().expect("coordinator address");
    let shutdown = CancellationToken::new();
    let server = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { coordinator.serve(listener, shutdown).await }
    });
    let client = reqwest::Client::new();
    let endpoint = format!("http://{address}/v1/identity/passkeys/registration/options");
    for bootstrap in &bootstraps[..64] {
        let response = client
            .post(&endpoint)
            .json(&RegistrationOptionsRequest {
                bootstrap_token: bootstrap,
                surface: "capacity_test",
            })
            .send()
            .await
            .expect("start ceremony");
        assert_eq!(response.status(), StatusCode::OK);
    }
    let rejected = client
        .post(&endpoint)
        .json(&RegistrationOptionsRequest {
            bootstrap_token: &bootstraps[64],
            surface: "capacity_test",
        })
        .send()
        .await
        .expect("reject excess ceremony");
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);

    rusqlite::Connection::open(&database)
        .expect("open identity database")
        .execute(
            "DELETE FROM passkey_registration_ceremonies
             WHERE ceremony_id = (SELECT ceremony_id FROM passkey_registration_ceremonies LIMIT 1)",
            [],
        )
        .expect("release one ceremony slot");
    let admitted = client
        .post(&endpoint)
        .json(&RegistrationOptionsRequest {
            bootstrap_token: &bootstraps[64],
            surface: "capacity_test",
        })
        .send()
        .await
        .expect("retry retained bootstrap");
    assert_eq!(admitted.status(), StatusCode::OK);

    shutdown.cancel();
    server
        .await
        .expect("coordinator task panicked")
        .expect("coordinator stopped cleanly");
}
