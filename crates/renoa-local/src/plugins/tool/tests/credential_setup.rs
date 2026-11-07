use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock, ToolCall, invoke_tool};
use renoa_control::{Coordinator, PeerIdentity};
use renoa_kernel::{CommandId, SessionId};
use renoa_rcp_wire::{ClientMessage, DeviceCredentials, JSON_WS_VERSION, NodeId, ServerMessage};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::super::{ManageTool, TOOL_NAME};

mod recovery;
use crate::{
    host::catalog,
    mcp::{McpAuthorizationResolver, McpCatalogStore, McpCredentialResolver, McpHostError},
    plugins::{
        PluginManager,
        tests::{test_agent_id, test_skill_store},
    },
};
use recovery::{duplicate_setup, replay, sign_in_again};

#[tokio::test]
async fn encrypted_browser_intake_finishes_before_the_connection_is_published() {
    run_credential_setup(
        "credential-test",
        "https://mcp.example/test",
        json!({
            "kind": "secret_service_bearer",
            "credential_id": "credential.test"
        }),
        Consent::Given,
    )
    .await;
}

#[tokio::test]
async fn a_setup_waiting_on_a_link_finishes_when_another_saves_the_credential() {
    run_credential_setup(
        "duplicate-credential",
        "https://mcp.example/duplicate",
        json!({
            "kind": "secret_service_bearer",
            "credential_id": "credential.duplicate"
        }),
        Consent::Duplicate,
    )
    .await;
}

#[tokio::test]
async fn without_a_setup_runner_the_turn_waits_for_the_person() {
    run_credential_setup(
        "unhanded-credential",
        "https://mcp.example/unhanded",
        json!({
            "kind": "secret_service_bearer",
            "credential_id": "credential.unhanded"
        }),
        Consent::Unhanded,
    )
    .await;
}

#[tokio::test]
async fn oauth_setup_is_selected_by_the_host_and_bound_to_the_discovered_issuer() {
    run_credential_setup(
        "oauth-credential-test",
        "https://mcp.example/oauth-test",
        json!({"kind": "oauth"}),
        Consent::Given,
    )
    .await;
}

#[tokio::test]
async fn a_sign_in_link_finishes_after_the_turn_that_showed_it_ends() {
    run_credential_setup(
        "callback-oauth",
        "https://mcp.example/callback",
        json!({"kind": "oauth"}),
        Consent::Callback,
    )
    .await;
}

#[tokio::test]
async fn a_refused_sign_in_comes_back_unfinished_and_publishes_nothing() {
    run_credential_setup(
        "refused-oauth",
        "https://mcp.example/refused",
        json!({"kind": "oauth"}),
        Consent::Refused,
    )
    .await;
}

#[tokio::test]
async fn an_expired_unpublished_connection_restarts_without_another_credential_submission() {
    run_credential_setup(
        "pending-oauth",
        "https://mcp.example/pending",
        json!({"kind":"oauth"}),
        Consent::Expires,
    )
    .await;
}

/// How the person answers the sign-in link.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Consent {
    /// No setup runner is attached, so the turn waits for the person.
    Unhanded,
    /// They ignore the credential link and answer a second setup's.
    Duplicate,
    /// The provider signs them in without a link.
    Given,
    /// They follow the link after the turn that showed it ended.
    Callback,
    /// They refuse it after the turn that showed it ended.
    Refused,
    /// They never do, and the link expires.
    Expires,
}

/// One setup under test, as the agent and the Host see it.
struct Run<'a> {
    tool: &'a ManageTool,
    sink: &'a CredentialSubmitter,
    arguments: &'a Value,
    directory: &'a std::path::Path,
    database: &'a std::path::Path,
    connection: &'a str,
    session: SessionId,
    command: CommandId,
}

async fn run_credential_setup(
    connection: &str,
    endpoint: &str,
    credential: Value,
    consent: Consent,
) {
    let directory = tempdir().expect("temporary credential setup fixture");
    let relay = CredentialRelayServer::start(&directory).await;

    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    crate::test_agents::insert_agent(&database, &test_agent_id(1).to_string());
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let adapter = directory.path().join("credential-adapter.mjs");
    write_credential_adapter(&adapter);
    if matches!(
        consent,
        Consent::Callback | Consent::Refused | Consent::Expires
    ) {
        fs::write(directory.path().join("wait-for-consent"), "wait").expect("pause authorization");
    }
    let manager = plugin_manager(&directory, &relay, &mcp, adapter);
    let (session, command) = (SessionId::new(), CommandId::new());
    let tool = ManageTool::for_session(
        test_agent_id(1),
        manager.clone(),
        directory.path().to_path_buf(),
        session,
        Some(command),
    );
    let sink = CredentialSubmitter {
        http: reqwest::Client::new(),
        catalog: mcp.clone(),
        connection: connection.to_owned(),
        credential_prompts: AtomicUsize::new(0),
        ignored_prompts: AtomicUsize::new(usize::from(consent == Consent::Duplicate)),
        sign_in: std::sync::Mutex::default(),
    };
    let arguments = json!({
        "action": "add",
        "source": {
            "kind": "mcp",
            "name": "credential-test",
            "description": "Credential setup fixture.",
            "server": "credential-test",
            "endpoint": endpoint,
            "documentation": "https://mcp.example/docs"
        },
        "credential": credential,
        "connection": connection
    });
    let run = Run {
        tool: &tool,
        sink: &sink,
        arguments: &arguments,
        directory: directory.path(),
        database: &database,
        connection,
        session,
        command,
    };
    let runner = (consent != Consent::Unhanded).then(|| manager.setups().attach());
    let result = call(&tool, &sink, "credential-connect", &arguments).await;
    if runner.is_none() {
        assert!(text(&result).contains("catalog_loaded"), "{result:?}");
        assert!(mcp.connection_config(connection).is_ok());
        relay.stop().await;
        return;
    }
    // The turn ends once the person has the link, with the call saved.
    assert_waiting(&result);
    assert!(
        mcp.connection_config(connection).is_err(),
        "nothing is published while the person has the link"
    );
    let operation = crate::mcp::oauth_operation_id(session, Some(command), "credential-connect");
    assert_eq!(saved_call(&database, &operation), arguments);
    // The Host's setup wait replays it, with no one to show a link to.
    let published = if consent == Consent::Duplicate {
        Box::pin(duplicate_setup(&run)).await
    } else if credential["kind"] == "oauth" {
        let replayed = replay(&run, "credential-connect").await;
        Box::pin(sign_in_again(&run, consent, replayed)).await
    } else {
        let finished = replay(&run, "credential-connect").await;
        let finished = finished.expect("the replay finishes the setup");
        assert!(finished.contains("catalog_loaded"), "{finished}");
        true
    };
    assert_eq!(sink.credential_prompts.load(Ordering::SeqCst), 1);
    let config = mcp.connection_config(connection);
    assert_eq!(config.is_ok(), published, "published only when finished");
    assert!(config.is_err() || config.is_ok_and(|config| config.endpoint == endpoint));
    relay.stop().await;
}

fn plugin_manager(
    directory: &tempfile::TempDir,
    relay: &CredentialRelayServer,
    mcp: &McpCatalogStore,
    adapter: std::path::PathBuf,
) -> PluginManager {
    let database = directory.path().join("host.sqlite3");
    let authorizations = McpAuthorizationResolver::with_remote_oauth(
        mcp,
        Some(adapter.clone()),
        McpCredentialResolver::default(),
        &relay.origin,
        &relay.credential_file,
        &directory.path().join("credentials"),
    )
    .expect("configure credential relay");
    PluginManager::initialize_with_authorizations(
        database.clone(),
        directory.path().join("installed"),
        mcp.clone(),
        Some(adapter),
        None,
        authorizations,
        test_skill_store(&database, directory.path()),
    )
    .expect("initialize plugin manager")
}

async fn call(
    tool: &ManageTool,
    sink: &CredentialSubmitter,
    id: &str,
    arguments: &Value,
) -> renoa_agent::ToolResult {
    let called = invoke_tool(
        Some(tool),
        ToolCall {
            id: id.to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: arguments.clone(),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        Some(sink),
    );
    // A call that waited on the person in its turn would hang here.
    tokio::time::timeout(Duration::from_secs(30), called)
        .await
        .expect("plugin_manage returns without waiting on the person")
        .expect("plugin_manage has a definite result")
}

/// The arguments of the call saved for the Host's setup wait under `operation`.
fn saved_call(database: &std::path::Path, operation: &str) -> Value {
    let saved: String = rusqlite::Connection::open(database)
        .expect("catalog")
        .query_row(
            "SELECT arguments FROM host_plugin_setups WHERE operation_id=?1",
            [operation],
            |row| row.get(0),
        )
        .expect("the call is saved for the Host's setup wait");
    serde_json::from_str(&saved).expect("saved call")
}

/// The call ended its turn with the person holding a setup link.
fn assert_waiting(result: &renoa_agent::ToolResult) {
    assert!(!result.is_error, "{result:?}");
    let waiting: Value = serde_json::from_str(&text(result)).expect("the tool's JSON");
    assert_eq!(waiting["status"], "waiting_for_person");
}

fn text(result: &renoa_agent::ToolResult) -> String {
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("one text block: {result:?}")
    };
    text.clone()
}

fn awaiting_callbacks(database: &std::path::Path, connection: &str) -> i64 {
    rusqlite::Connection::open(database)
        .expect("Host database")
        .query_row(
            "SELECT count(*) FROM mcp_oauth_flows WHERE connection_id=?1 AND phase='awaiting_callback'",
            [connection],
            |row| row.get(0),
        )
        .expect("OAuth flows")
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
    connection: String,
    /// Credential links the person answered.
    credential_prompts: AtomicUsize,
    /// Credential links the person leaves unanswered, first ones first.
    ignored_prompts: AtomicUsize,
    /// The last sign-in link shown.
    sign_in: std::sync::Mutex<Option<String>>,
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
            if value["status"] == "authorization_required" {
                *self.sign_in.lock().expect("sign-in link") =
                    value["authorization_url"].as_str().map(str::to_owned);
                return;
            }
            if value["status"] != "credential_required" {
                return;
            }
            assert!(matches!(
                self.catalog.connection_config(&self.connection),
                Err(McpHostError::NotFound(_))
            ));
            let ignored =
                self.ignored_prompts
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                        left.checked_sub(1)
                    });
            if ignored.is_ok() {
                return;
            }
            self.credential_prompts.fetch_add(1, Ordering::SeqCst);
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
    let issuer = values.get("issuer").map(String::as_str).unwrap_or_default();
    let nonce = [5_u8; 12];
    let mut ciphertext = if kind == "oauth_client" {
        assert_eq!(issuer, "https://accounts.example");
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "issuer": issuer,
            "client_id": "browser-client",
            "client_secret": "browser-secret"
        }))
        .expect("encode OAuth client")
    } else {
        br#"{"schema_version":1,"value":"browser-secret"}"#.to_vec()
    };
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).expect("valid setup key"))
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(format!(
                "renoa credential relay v2\0{relay_id}\0{credential_id}\0{kind}\0{issuer}"
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

async fn enroll(endpoint: &str, token: renoa_rcp_wire::EnrollmentToken) -> DeviceCredentials {
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
import fs from 'node:fs';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
if (request.action === 'oauth_discover') {
  send({
    wire_version: 9,
    event: 'oauth_discovered',
    discovery: {
      mcp_endpoint: request.endpoint,
      issuer: 'https://accounts.example',
      client_id_metadata_document_supported: false,
      dynamic_client_registration_supported: false
    }
  });
  process.exit(0);
}
if (request.action === 'oauth_token') {
  if (request.oauth_state.access_token !== 'oauth-access') {
    send({wire_version:9,event:'oauth_refresh_required',oauth_state:request.oauth_state});
    process.exit(0);
  }
  send({wire_version:9,event:'oauth_authorized',authorization:{scheme:'bearer',token:'oauth-access'},oauth_state:request.oauth_state});
  process.exit(0);
}
if (request.action === 'oauth_exchange') {
  if (request.authorization_code !== 'sign-in-code' ||
      request.registration?.client_id !== 'browser-client' ||
      request.registration?.client_secret !== 'browser-secret') process.exit(23);
  send({wire_version:9,event:'oauth_authorized',authorization:{scheme:'bearer',token:'oauth-access'},oauth_state:{...request.oauth_state,access_token:'oauth-access'}});
  process.exit(0);
}
if (request.action === 'oauth_begin') {
  fs.appendFileSync(new URL('./oauth-begins', import.meta.url), 'begin\n');
  if (fs.existsSync(new URL('./wait-for-consent', import.meta.url))) {
    const authorization = new URL('https://accounts.example/authorize');
    authorization.searchParams.set('state',request.csrf_state);
    authorization.searchParams.set('redirect_uri',request.redirect_uri);
    send({wire_version:9,event:'oauth_redirect',authorization_url:authorization.href,oauth_state:{
      schema_version:1,mcp_endpoint:request.endpoint,csrf_state:request.csrf_state,
      redirect_uri:request.redirect_uri,authorization_url:authorization.href
    }});
    process.exit(0);
  }

  if (request.registration?.mode !== 'pre_registered' ||
      request.registration?.issuer !== 'https://accounts.example' ||
      request.registration?.client_id !== 'browser-client' ||
      request.registration?.client_secret !== 'browser-secret') process.exit(22);
  send({
    wire_version: 9,
    event: 'oauth_authorized',
    authorization: {scheme: 'bearer', token: 'oauth-access'},
    oauth_state: {
      schema_version: 1,
      mcp_endpoint: request.endpoint,
      csrf_state: request.csrf_state,
      redirect_uri: request.redirect_uri,
      access_token: 'oauth-access'
    }
  });
  process.exit(0);
}
if (request.action !== 'discover' ||
    request.credential?.name !== 'authorization' ||
    request.credential?.prefix !== 'Bearer ' ||
    !['browser-secret', 'oauth-access'].includes(request.credential?.secret)) process.exit(21);
send({
  wire_version: 9,
  event: 'discovered',
  catalog: {
    endpoint: request.endpoint,
    protocol_version: '2026-07-28',
    adapter_revision: 'mcp-client-node-v0.10.0',
    tools: [{
      name: 'credential_test',
      description: 'Proves secure credential intake.',
      input_schema: {type: 'object'},
      model_input_schema: {type: 'object'}
    }],
    rejected_tools: []
  }
});
",
    )
    .expect("write credential adapter");
}

fn decode<const N: usize>(value: &str) -> [u8; N] {
    let mut output = [0_u8; N];
    for (slot, pair) in output.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
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
