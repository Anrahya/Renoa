use std::fs;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::super::{
    process::{self, OAuthRegistration, OAuthResult},
    secret::OAuthSecretStore,
    store::{OAuthFlow, OAuthPhase},
};
use super::support::{ENDPOINT, Fixture, compile_secret_tool};
use super::{authorization_request, support::CONNECTION};
use crate::mcp::{McpConnectionAuth, McpHostError, McpOAuthError, McpOAuthRegistration};

#[tokio::test]
async fn pre_registered_oauth_clients_are_loaded_from_one_named_secret_reference() {
    let directory = tempdir().expect("temporary OAuth client credential fixture");
    let data = directory.path().join("credential.json");
    let writes = directory.path().join("writes");
    fs::write(
        &data,
        r#"{"schema_version":1,"issuer":"https://accounts.example","client_id":"drive-client","client_secret":"drive-secret"}"#,
    )
    .expect("write OAuth client credential");
    let executable = compile_secret_tool(directory.path(), &data, &writes);
    let store = OAuthSecretStore::new(executable);

    let client = store
        .load_pre_registered_client("drive.client", CancellationToken::new())
        .await
        .expect("load pre-registered client");
    assert_eq!(client.issuer, "https://accounts.example");
    assert_eq!(client.client_id, "drive-client");
    assert_eq!(
        client
            .client_secret
            .as_ref()
            .expect("fixture has client secret")
            .expose(),
        "drive-secret"
    );

    fs::write(
        &data,
        r#"{"schema_version":1,"issuer":"https://accounts.example","client_id":"drive-client","client_secret":"drive-secret","unexpected":true}"#,
    )
    .expect("write malformed OAuth client credential");
    let result = store
        .load_pre_registered_client("drive.client", CancellationToken::new())
        .await;
    let Err(error) = result else {
        panic!("unknown credential fields must fail closed")
    };
    assert!(!error.to_string().contains("drive-secret"));
}

#[tokio::test]
async fn recovery_that_cannot_mutate_oauth_does_not_require_the_client_secret() {
    let fixture = Fixture::new();
    let auth = McpConnectionAuth::oauth(
        CONNECTION,
        ENDPOINT,
        McpOAuthRegistration::pre_registered("missing.client").expect("credential reference"),
    )
    .expect("pre-registered OAuth reference");
    fixture
        .resolver
        .oauth
        .flows
        .put(
            &OAuthFlow::non_interactive(CONNECTION, "prior-operation", OAuthPhase::Unknown)
                .expect("unknown flow"),
        )
        .await
        .expect("store unknown flow");

    let result = fixture
        .resolver
        .authorize(
            authorization_request(&auth, "recovery-operation", false),
            CancellationToken::new(),
        )
        .await;

    assert!(matches!(
        result,
        Err(McpHostError::OAuth(McpOAuthError::OutcomeUnknown { .. }))
    ));
}

#[tokio::test]
async fn host_wire_carries_all_three_oauth_registration_modes_exactly() {
    let directory = tempdir().expect("temporary registration wire fixture");
    let requests = directory.path().join("registrations.jsonl");
    let adapter = directory.path().join("registration-adapter.mjs");
    let requests_json =
        serde_json::to_string(&requests.to_string_lossy()).expect("encode request log path");
    fs::write(
        &adapter,
        format!(
            r"
import fs from 'node:fs';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
fs.appendFileSync({requests_json}, `${{JSON.stringify(request.registration)}}\n`);
process.stdout.write(`${{JSON.stringify({{
  wire_version: 6,
  event: 'oauth_failed',
  failure: {{
    kind: 'protocol',
    certainty: 'definite',
    message: 'fixture stopped before authorization',
    partial_changes_possible: false,
    diagnostic: {{code: 'fixture_stop', detail: 'registration captured'}}
  }},
  oauth_state: {{
    schema_version: 1,
    mcp_endpoint: request.endpoint,
    csrf_state: request.csrf_state,
    redirect_uri: request.redirect_uri
  }}
}})}}\n`);
"
        ),
    )
    .expect("write registration adapter");
    let modes = [
        OAuthRegistration::Dynamic,
        OAuthRegistration::ClientMetadata {
            client_metadata_url: "https://renoa.example/oauth/client.json".to_owned(),
        },
        OAuthRegistration::PreRegistered {
            issuer: "https://accounts.example".to_owned(),
            client_id: "client-one".to_owned(),
            client_secret: Some(
                serde_json::from_value(serde_json::json!("secret-one"))
                    .expect("sensitive test value"),
            ),
        },
    ];
    for (index, registration) in modes.iter().enumerate() {
        let result = process::begin(
            &adapter,
            process::OAuthBegin {
                endpoint: ENDPOINT,
                csrf_state: &format!("state-{index}"),
                redirect_uri: &format!("http://127.0.0.1:{}/oauth/callback", 41000 + index),
                force_reauthorization: false,
                registration,
                prior: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect("registration reaches adapter");
        assert!(matches!(result, OAuthResult::Failed { .. }));
    }
    let observed = fs::read_to_string(requests)
        .expect("read registration log")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("registration JSON"))
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            serde_json::json!({"mode": "dynamic"}),
            serde_json::json!({
                "mode": "client_metadata",
                "client_metadata_url": "https://renoa.example/oauth/client.json"
            }),
            serde_json::json!({
                "mode": "pre_registered",
                "issuer": "https://accounts.example",
                "client_id": "client-one",
                "client_secret": "secret-one"
            }),
        ]
    );
}
