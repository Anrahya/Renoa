use renoa_control::Coordinator;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn hosted_client_metadata_is_public_and_uses_the_exact_callback() {
    let files = TempDir::new().expect("temporary control directory");
    let coordinator = Coordinator::open_with_passkeys(
        files.path().join("control.sqlite"),
        "renoa.example",
        "https://renoa.example",
    )
    .expect("open coordinator");
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind coordinator");
    let address = listener.local_addr().expect("coordinator address");
    let shutdown = CancellationToken::new();
    let server_shutdown = shutdown.clone();
    let task = tokio::spawn(async move {
        coordinator
            .serve(listener, server_shutdown)
            .await
            .expect("serve coordinator");
    });

    let response = reqwest::Client::new()
        .get(format!("http://{address}/v1/oauth/client-metadata.json"))
        .send()
        .await
        .expect("load client metadata without node authentication");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.headers()[reqwest::header::CACHE_CONTROL],
        "no-store"
    );
    let metadata: serde_json::Value = response.json().await.expect("decode client metadata");
    assert_eq!(
        metadata,
        serde_json::json!({
            "client_id": "https://renoa.example/v1/oauth/client-metadata.json",
            "redirect_uris": ["https://renoa.example/v1/oauth/callback"],
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "application_type": "web",
            "client_name": "Renoa",
            "client_uri": "https://renoa.example",
            "software_id": "renoa",
            "software_version": env!("CARGO_PKG_VERSION")
        })
    );

    shutdown.cancel();
    task.await.expect("coordinator task joins");
}
