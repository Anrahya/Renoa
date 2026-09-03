use std::{
    fs,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock, ToolCall, invoke_tool};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, JSON_WS_VERSION, NodeId, PeerIdentity,
    ServerMessage,
};
use renoa_kernel::{CommandId, SessionId};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::super::{ManageTool, TOOL_NAME};
use crate::{
    ALPHA_PROFILE_ID, AgentProfileId,
    host::catalog,
    mcp::{McpAuthorizationResolver, McpCatalogStore, McpCredentialResolver, McpHostError},
    plugins::{PluginManager, tests::test_skill_store},
};

#[tokio::test]
async fn encrypted_browser_intake_finishes_before_the_connection_is_published() {
    let directory = tempdir().expect("temporary credential setup fixture");
    let relay = CredentialRelayServer::start(&directory).await;

    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let adapter = directory.path().join("credential-adapter.mjs");
    write_credential_adapter(&adapter);
    let authorizations = McpAuthorizationResolver::with_remote_oauth(
        &mcp,
        Some(adapter.clone()),
        McpCredentialResolver::default(),
        &relay.origin,
        &relay.credential_file,
    )
    .expect("configure credential relay");
    let manager = PluginManager::initialize_with_authorizations(
        database.clone(),
        directory.path().join("installed"),
        mcp.clone(),
        Some(adapter),
        None,
        authorizations,
        test_skill_store(&database, directory.path()),
    )
    .expect("initialize extension manager");
    let tool = ManageTool::for_session(
        AgentProfileId::new(ALPHA_PROFILE_ID).expect("valid Alpha profile"),
        manager,
        directory.path().to_path_buf(),
        SessionId::new(),
        Some(CommandId::new()),
    );
    let sink = CredentialSubmitter {
        http: reqwest::Client::new(),
        catalog: mcp.clone(),
    };
    let result = invoke_tool(
        Some(&tool),
        ToolCall {
            id: "credential-connect".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: json!({
                "action": "add",
                "source": {
                    "kind": "mcp",
                    "name": "credential-test",
                    "description": "Credential setup fixture.",
                    "server": "credential-test",
                    "endpoint": "https://mcp.example/test",
                    "documentation": "https://mcp.example/docs"
                },
                "credential": {
                    "kind": "secret_service_bearer",
                    "credential_id": "credential.test"
                },
                "connection": "credential-test"
            }),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        Some(&sink),
    )
    .await
    .expect("credential setup has a definite result");
    assert!(!result.is_error, "credential setup failed: {result:?}");
    assert_eq!(
        mcp.connection_config("credential-test")
            .expect("connection is published only after setup")
            .endpoint,
        "https://mcp.example/test"
    );
    relay.stop().await;
}

struct CredentialRelayServer {
    origin: String,
    credential_file: std::path::PathBuf,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl CredentialRelayServer {
    async fn start(directory: &tempfile::TempDir) -> Self {
        let database = directory.path().join("control.sqlite3");
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind coordinator");
        let address = listener.local_addr().expect("coordinator address");
        let origin = format!("http://localhost:{}", address.port());
        let coordinator = Coordinator::open_with_passkeys(&database, "localhost", &origin)
            .expect("open coordinator");
        let enrollment = coordinator
            .create_enrollment(
                PeerIdentity::Node {
                    node_id: NodeId::new(),
                },
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create Host enrollment");
        let shutdown = CancellationToken::new();
        let server_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            coordinator
                .serve(listener, server_shutdown)
                .await
                .expect("serve coordinator");
        });
        let credentials = enroll(&format!("ws://{address}/connect"), enrollment).await;
        let credential_file = directory.path().join("relay-device.json");
        fs::write(
            &credential_file,
            serde_json::to_vec(&credentials).expect("encode relay credentials"),
        )
        .expect("write relay credentials");
        make_private(&credential_file);
        Self {
            origin,
            credential_file,
            shutdown,
            task,
        }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.expect("coordinator task joins");
    }
}

struct CredentialSubmitter {
    http: reqwest::Client,
    catalog: McpCatalogStore,
}

impl AgentEventSink for CredentialSubmitter {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let AgentEvent::ToolExecutionUpdate { update, .. } = event else {
                return;
            };
            let [ContentBlock::Text { text }] = update.content.as_slice() else {
                return;
            };
            let Ok(value) = serde_json::from_str::<Value>(text) else {
                return;
            };
            if value["status"] != "credential_required" {
                return;
            }
            assert!(matches!(
                self.catalog.connection_config("credential-test"),
                Err(McpHostError::NotFound(_))
            ));
            submit_encrypted(&self.http, &value).await;
        })
    }
}

async fn submit_encrypted(http: &reqwest::Client, update: &Value) {
    let setup = Url::parse(
        update["setup_url"]
            .as_str()
            .expect("credential update has setup URL"),
    )
    .expect("valid setup URL");
    let fragment = setup.fragment().expect("setup URL has secrets");
    let values = url::form_urlencoded::parse(fragment.as_bytes())
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    let key = decode::<32>(values.get("key").expect("setup key"));
    let capability = values.get("token").expect("setup capability");
    let relay_id = setup
        .path_segments()
        .and_then(|mut segments| segments.nth_back(1))
        .expect("relay id in setup URL");
    let credential_id = update["credential"].as_str().expect("credential id");
    let kind = update["credential_kind"].as_str().expect("credential kind");
    let nonce = [5_u8; 12];
    let mut ciphertext = br#"{"schema_version":1,"value":"browser-secret"}"#.to_vec();
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).expect("valid setup key"))
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(format!(
                "renoa credential relay v1\0{relay_id}\0{credential_id}\0{kind}"
            )),
            &mut ciphertext,
        )
        .expect("encrypt credential");
    let mut submit = setup.clone();
    submit.set_fragment(None);
    submit.set_path(&format!("/v1/credential-relays/{relay_id}/submit"));
    let response = http
        .post(submit)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(
            serde_json::to_vec(&json!({
                "version": 1,
                "capability": capability,
                "nonce": hex(&nonce),
                "ciphertext": hex(&ciphertext)
            }))
            .expect("encode credential submission"),
        )
        .send()
        .await
        .expect("submit encrypted credential");
    assert!(response.status().is_success());
}

async fn enroll(endpoint: &str, token: renoa_control::EnrollmentToken) -> DeviceCredentials {
    let (mut socket, _) = connect_async(endpoint).await.expect("connect enrollment");
    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage::Enroll {
                version: JSON_WS_VERSION,
                token,
            })
            .expect("encode enrollment")
            .into(),
        ))
        .await
        .expect("send enrollment");
    let Message::Text(text) = socket
        .next()
        .await
        .expect("enrollment response")
        .expect("valid enrollment response")
    else {
        panic!("enrollment response must be text")
    };
    let ServerMessage::Enrolled { credentials, .. } =
        serde_json::from_str(&text).expect("decode enrollment response")
    else {
        panic!("coordinator must issue credentials")
    };
    credentials
}

fn write_credential_adapter(path: &std::path::Path) {
    fs::write(
        path,
        r"
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
if (request.credential?.name !== 'authorization' ||
    request.credential?.prefix !== 'Bearer ' ||
    request.credential?.secret !== 'browser-secret') process.exit(21);
process.stdout.write(JSON.stringify({
  wire_version: 8,
  event: 'discovered',
  catalog: {
    endpoint: request.endpoint,
    protocol_version: '2026-07-28',
    adapter_revision: 'mcp-client-node-v0.8.0',
    tools: [{
      name: 'credential_test',
      description: 'Proves secure credential intake.',
      input_schema: {type: 'object'},
      model_input_schema: {type: 'object'}
    }],
    rejected_tools: []
  }
}) + '\n');
",
    )
    .expect("write credential adapter");
}

fn decode<const N: usize>(value: &str) -> [u8; N] {
    let mut output = [0_u8; N];
    for (slot, pair) in output.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *slot = (digit(pair[0]) << 4) | digit(pair[1]);
    }
    output
}

fn digit(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => panic!("fixture hex must be lowercase"),
    }
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(unix)]
fn make_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("restrict credentials");
}

#[cfg(not(unix))]
fn make_private(_path: &std::path::Path) {}
