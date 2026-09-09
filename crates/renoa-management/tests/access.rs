use std::{
    net::SocketAddr,
    path::Path,
    time::{Duration, SystemTime},
};

use renoa_control::{BrowserSessions, Coordinator};
use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, alpha_profile,
};
use renoa_management::ManagementApi;
use renoa_protocol::PrincipalId;
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};

struct IdentityServer {
    address: SocketAddr,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), renoa_control::ControlError>>,
}

fn host(root: &Path) -> LocalHost {
    LocalHost::new(
        root,
        LocalModelConfiguration::new(
            root.join("absent-model"),
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "absent-model",
            root.join("absent-credentials"),
        ),
        vec![alpha_profile()],
        LocalHostAdapters::default(),
    )
    .expect("Host initialized without provider access")
}

async fn register(
    client: &Client,
    address: SocketAddr,
    control: &Coordinator,
    principal: PrincipalId,
) -> String {
    let bootstrap = control
        .create_passkey_bootstrap(principal, SystemTime::now() + Duration::from_mins(5))
        .await
        .expect("bootstrap");
    let options: Value = client
        .post(format!(
            "http://{address}/v1/identity/passkeys/registration/options"
        ))
        .json(&json!({"bootstrapToken":bootstrap,"surface":"control_room"}))
        .send()
        .await
        .expect("options")
        .error_for_status()
        .expect("valid options")
        .json()
        .await
        .expect("options JSON");
    let mut auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = auth
        .do_registration(
            url::Url::parse("http://localhost").expect("origin"),
            serde_json::from_value(options["options"].clone()).expect("WebAuthn options"),
        )
        .expect("register");
    let response = client
        .post(format!(
            "http://{address}/v1/identity/passkeys/registration/verify"
        ))
        .json(&json!({"ceremonyId":options["ceremonyId"],"credential":credential}))
        .send()
        .await
        .expect("verify")
        .error_for_status()
        .expect("valid passkey");
    response.headers()["set-cookie"]
        .to_str()
        .expect("cookie")
        .split(';')
        .next()
        .expect("pair")
        .to_owned()
}

#[tokio::test]
async fn real_passkey_owner_is_required_and_outages_do_not_become_logout() {
    let files = tempfile::tempdir().expect("directory");
    let root = files.path().join("host");
    let host = host(&root);
    let id = host.host_id().await.expect("Host identity");
    let database = files.path().join("identity.sqlite");
    let control = Coordinator::open_with_passkeys(&database, "localhost", "http://localhost")
        .expect("coordinator");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("identity listener");
    let identity = listener.local_addr().expect("address");
    let identity_stop = CancellationToken::new();
    let identity_task = tokio::spawn(control.clone().serve(listener, identity_stop.clone()));
    let owner = PrincipalId::from_uuid(Uuid::new_v4());
    let client = Client::new();
    let cookie = register(&client, identity, &control, owner).await;
    let wrong_cookie = register(
        &client,
        identity,
        &control,
        PrincipalId::from_uuid(Uuid::new_v4()),
    )
    .await;
    assert!(ManagementApi::open(&root, Uuid::new_v4(), identity, owner).is_err());
    assert!(
        ManagementApi::open(&root, id, "0.0.0.0:7818".parse().expect("address"), owner).is_err()
    );
    let assets = files.path().join("assets");
    std::fs::create_dir(&assets).expect("asset directory");
    std::fs::write(assets.join("index.html"), "control panel").expect("app shell");
    let api = ManagementApi::open(&root, id, identity, owner)
        .expect("bind exact Host")
        .with_assets(&assets)
        .expect("public assets");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("management listener");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let stop = CancellationToken::new();
    let task = tokio::spawn(api.serve(listener, stop.clone()));
    check_public_shell_and_private_detail(&client, &url, &cookie).await;
    let request = || client.get(format!("{url}/v1/host"));
    assert_eq!(
        request().send().await.expect("anonymous").status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request()
            .header("x-renoa-principal", owner.to_string())
            .send()
            .await
            .expect("spoof")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request()
            .header("cookie", wrong_cookie)
            .send()
            .await
            .expect("wrong owner")
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = request()
        .header("cookie", &cookie)
        .send()
        .await
        .expect("owner observation");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let snapshot: Value = response.json().await.expect("snapshot");
    assert_eq!(snapshot["host_id"], id.to_string());
    assert_eq!(snapshot["agents"], json!([]));
    assert!(!root.join("absent-model").exists());
    assert!(!root.join("absent-credentials").exists());

    check_outage_and_revocation(
        &client,
        &url,
        &cookie,
        &root,
        &database,
        owner,
        IdentityServer {
            address: identity,
            stop: identity_stop,
            task: identity_task,
        },
    )
    .await;
    stop.cancel();
    task.await
        .expect("management task")
        .expect("management stop");
}

async fn check_public_shell_and_private_detail(client: &Client, url: &str, cookie: &str) {
    let shell = client.get(url).send().await.expect("app shell");
    assert_eq!(shell.status(), StatusCode::OK);
    assert_eq!(shell.text().await.expect("shell body"), "control panel");
    let detail = format!("{url}/v1/host/reviews/{}", Uuid::new_v4());
    assert_eq!(
        client
            .get(&detail)
            .send()
            .await
            .expect("anonymous details")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(&detail)
            .header("cookie", cookie)
            .send()
            .await
            .expect("absent review")
            .status(),
        StatusCode::NOT_FOUND
    );
    for path in ["/v1/unknown", "/assets/unknown.js"] {
        assert_eq!(
            client
                .get(format!("{url}{path}"))
                .header("accept", "text/html")
                .send()
                .await
                .expect("missing path")
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        client
            .post(url)
            .send()
            .await
            .expect("write to shell")
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
}

async fn check_outage_and_revocation(
    client: &Client,
    url: &str,
    cookie: &str,
    root: &Path,
    database: &Path,
    owner: PrincipalId,
    identity: IdentityServer,
) {
    let request = || client.get(format!("{url}/v1/host"));

    identity.stop.cancel();
    identity
        .task
        .await
        .expect("identity task")
        .expect("identity stop");
    assert_eq!(
        request()
            .header("cookie", cookie)
            .send()
            .await
            .expect("identity outage")
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let restarted = Coordinator::open_with_passkeys(database, "localhost", "http://localhost")
        .expect("identity restart");
    let listener = TcpListener::bind(identity.address)
        .await
        .expect("same identity address");
    let restarted_stop = CancellationToken::new();
    let restarted_task = tokio::spawn(restarted.serve(listener, restarted_stop.clone()));
    assert_eq!(
        request()
            .header("cookie", cookie)
            .send()
            .await
            .expect("automatic recovery")
            .status(),
        StatusCode::OK
    );
    // The same configured root must not silently become another Host after startup.
    rusqlite::Connection::open(root.join("host.sqlite3"))
        .expect("Host catalog")
        .execute(
            "UPDATE host_identity SET host_id=?1",
            [Uuid::new_v4().to_string()],
        )
        .expect("identity replacement");
    assert_eq!(
        request()
            .header("cookie", cookie)
            .send()
            .await
            .expect("changed Host")
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    BrowserSessions::open(database)
        .expect("identity admin")
        .revoke(owner)
        .await
        .expect("revoke owner logins");
    assert_eq!(
        request()
            .header("cookie", cookie)
            .send()
            .await
            .expect("revoked login")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    restarted_stop.cancel();
    restarted_task
        .await
        .expect("identity task")
        .expect("identity stop");
}
