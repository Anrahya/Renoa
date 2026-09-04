use std::fs;

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::super::{
    automatic_client_credential_id,
    process::{self, OAuthRegistration, OAuthResult},
    secret_store::OAuthSecretStore,
    select_automatic_registration,
    store::{OAuthFlow, OAuthPhase},
};
use super::support::{ENDPOINT, Fixture, compile_secret_tool};
use super::{authorization_request, support::CONNECTION};
use crate::mcp::{McpConnectionAuth, McpHostError, McpOAuthError, McpOAuthRegistration};

#[test]
fn automatic_oauth_registration_follows_verified_server_metadata() {
    let discovery = process::OAuthDiscovery {
        issuer: "https://accounts.example".to_owned(),
        client_metadata_supported: true,
        dynamic_registration_supported: true,
    };
    let credential_id = automatic_client_credential_id(&discovery.issuer);

    assert_eq!(
        select_automatic_registration(
            &discovery,
            Some("https://renoa.example/v1/oauth/client-metadata.json"),
            &credential_id,
            true,
        )
        .expect("saved provider client wins"),
        McpOAuthRegistration::pre_registered_for_issuer(&credential_id, "https://accounts.example")
            .expect("expected pre-registered policy")
    );
    assert_eq!(
        select_automatic_registration(
            &discovery,
            Some("https://renoa.example/v1/oauth/client-metadata.json"),
            &credential_id,
            false,
        )
        .expect("advertised CIMD wins"),
        McpOAuthRegistration::client_metadata(
            "https://renoa.example/v1/oauth/client-metadata.json"
        )
        .expect("expected CIMD policy")
    );

    let dynamic_only = process::OAuthDiscovery {
        client_metadata_supported: false,
        ..discovery.clone()
    };
    assert_eq!(
        select_automatic_registration(&dynamic_only, None, &credential_id, false)
            .expect("advertised DCR is selected"),
        McpOAuthRegistration::dynamic()
    );

    let developer_app = process::OAuthDiscovery {
        client_metadata_supported: false,
        dynamic_registration_supported: false,
        ..discovery
    };
    assert_eq!(
        select_automatic_registration(&developer_app, None, &credential_id, false)
            .expect("unsupported automatic registration requests a provider-bound client"),
        McpOAuthRegistration::pre_registered_for_issuer(&credential_id, "https://accounts.example")
            .expect("expected provider-bound client")
    );
}

#[tokio::test]
async fn oauth_metadata_discovery_uses_its_own_strict_wire_action() {
    let directory = tempdir().expect("temporary OAuth discovery fixture");
    let adapter = directory.path().join("discovery-adapter.mjs");
    fs::write(
        &adapter,
        r"
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
if (request.action !== 'oauth_discover' || request.wire_version !== 9 ||
    request.endpoint !== 'https://mcp.example.test/mcp') process.exit(17);
process.stdout.write(JSON.stringify({
  wire_version: 9,
  event: 'oauth_discovered',
  discovery: {
    mcp_endpoint: request.endpoint,
    issuer: 'https://accounts.example/',
    client_id_metadata_document_supported: true,
    dynamic_client_registration_supported: false
  }
}) + '\n');
",
    )
    .expect("write discovery adapter");

    let discovery = process::discover(&adapter, ENDPOINT, CancellationToken::new())
        .await
        .expect("discover OAuth metadata");
    assert_eq!(discovery.issuer, "https://accounts.example");
    assert!(discovery.client_metadata_supported);
    assert!(!discovery.dynamic_registration_supported);
}

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
    let store = OAuthSecretStore::service(executable);

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
fs.appendFileSync({requests_json}, `${{JSON.stringify({{
  registration: request.registration,
  requested_scope: request.requested_scope ?? null
}})}}\n`);
process.stdout.write(`${{JSON.stringify({{
  wire_version: 9,
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
                requested_scope: (index == 0).then_some("items.read items.write"),
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
            serde_json::json!({
                "registration": {"mode": "dynamic"},
                "requested_scope": "items.read items.write"
            }),
            serde_json::json!({
                "registration": {
                    "mode": "client_metadata",
                    "client_metadata_url": "https://renoa.example/oauth/client.json"
                },
                "requested_scope": null
            }),
            serde_json::json!({
                "registration": {
                    "mode": "pre_registered",
                    "issuer": "https://accounts.example",
                    "client_id": "client-one",
                    "client_secret": "secret-one"
                },
                "requested_scope": null
            }),
        ]
    );
}
