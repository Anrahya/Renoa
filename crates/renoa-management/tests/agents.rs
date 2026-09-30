use renoa_control::{BrowserSessions, Coordinator};
use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, TurnObservation,
};
use renoa_management::{ManagementApi, ManagementError};
use renoa_protocol::PrincipalId;
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost";
struct Quiet;
impl renoa_agent::AgentEventSink for Quiet {
    fn emit(&self, _: renoa_agent::AgentEvent) -> renoa_agent::BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct Fixture {
    files: tempfile::TempDir,
    host: LocalHost,
    owner: PrincipalId,
    identity: SocketAddr,
    identity_stop: CancellationToken,
    identity_task: tokio::task::JoinHandle<Result<(), renoa_control::ControlError>>,
    url: String,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), ManagementError>>,
    cookie: String,
    client: Client,
}

impl Fixture {
    async fn new() -> Self {
        let files = tempfile::tempdir().unwrap();
        let root = files.path().join("home");
        let bridge = files.path().join("model.mjs");
        std::fs::write(&bridge, MODEL).unwrap();
        let host = LocalHost::new(
            &root,
            LocalModelConfiguration::new(
                &bridge,
                vec![ModelProvider::Xai],
                ModelProvider::Xai,
                "fixture-model",
                root.join("credentials/models.sqlite3"),
            ),
            LocalHostAdapters::default(),
        )
        .unwrap();
        std::fs::write(root.join("credentials/models.sqlite3"), "").unwrap();
        let database = files.path().join("identity.sqlite3");
        let coordinator = Coordinator::open_with_passkeys(&database, "localhost", ORIGIN).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let identity = listener.local_addr().unwrap();
        let identity_stop = CancellationToken::new();
        let identity_task = tokio::spawn(coordinator.serve(listener, identity_stop.clone()));
        let owner = PrincipalId::from_uuid(Uuid::new_v4());
        let client = Client::new();
        let cookie = pair(&client, &database, identity, owner).await;
        let api = ManagementApi::open(
            &root,
            host.host_id().await.unwrap(),
            identity,
            owner,
            ORIGIN,
        )
        .unwrap()
        .with_agent_creation(host.clone())
        .await
        .unwrap();
        let (url, stop, task) = serve(api).await;
        Self {
            files,
            host,
            owner,
            identity,
            identity_stop,
            identity_task,
            url,
            stop,
            task,
            cookie,
            client,
        }
    }
    async fn post(&self, body: &Value) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/host/agents", self.url))
            .header("origin", ORIGIN)
            .header("cookie", &self.cookie)
            .json(body)
            .send()
            .await
            .unwrap()
    }
    async fn profile(&self) -> reqwest::Response {
        self.client
            .get(format!("{}/v1/host/profile", self.url))
            .header("cookie", &self.cookie)
            .send()
            .await
            .unwrap()
    }
    async fn save_profile(&self, origin: &str, body: &Value) -> reqwest::Response {
        self.client
            .put(format!("{}/v1/host/profile", self.url))
            .header("origin", origin)
            .header("cookie", &self.cookie)
            .json(body)
            .send()
            .await
            .unwrap()
    }
    async fn restart(&mut self) {
        self.stop.cancel();
        (&mut self.task).await.unwrap().unwrap();
        let api = ManagementApi::open(
            &self.files.path().join("home"),
            self.host.host_id().await.unwrap(),
            self.identity,
            self.owner,
            ORIGIN,
        )
        .unwrap()
        .with_agent_creation(self.host.clone())
        .await
        .unwrap();
        (self.url, self.stop, self.task) = serve(api).await;
    }
    async fn close(self) {
        self.stop.cancel();
        self.task.await.unwrap().unwrap();
        self.identity_stop.cancel();
        self.identity_task.await.unwrap().unwrap();
    }
}

async fn serve(
    api: ManagementApi,
) -> (
    String,
    CancellationToken,
    tokio::task::JoinHandle<Result<(), ManagementError>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let task = tokio::spawn(api.serve(listener, stop.clone()));
    (url, stop, task)
}
async fn pair(
    client: &Client,
    database: &std::path::Path,
    identity: SocketAddr,
    owner: PrincipalId,
) -> String {
    let token = BrowserSessions::open(database)
        .unwrap()
        .create_pairing(owner, SystemTime::now() + Duration::from_mins(30))
        .await
        .unwrap();
    let response = client
        .post(format!("http://{identity}/v1/identity/pair"))
        .header("origin", ORIGIN)
        .json(&json!({"pairingToken":token,"browserNonce":"67".repeat(32)}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}
fn creation() -> Value {
    json!({ "operation_id":Uuid::new_v4(), "name":"Desk", "instructions":"Carry out the owner creation proof.", "tools":["read_file"], "connections":[], "preset_id":null, "automation":null, "behavior":null, "documents":{"soul":true,"user":true}, "model":{"provider":"xai","model":"fixture-model","reasoning":"high"} })
}

#[tokio::test]
async fn owner_creation_is_usable_and_identical_retry_survives_a_lost_reply_and_restart() {
    let mut f = Fixture::new().await;
    let options: Value = f
        .client
        .get(format!("{}/v1/host/agents/options", f.url))
        .header("cookie", &f.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(options["native_tools"].is_array(), "{options}");
    assert_eq!(options["native_tools"].as_array().unwrap().len(), 6);
    assert_eq!(options["default_model"]["provider"], "xai");
    assert_eq!(options["models"][0]["reasoning_levels"], json!(["high"]));
    let mut body = creation();
    body["instructions"] = json!("Follow the owner instructions. ".repeat(300));
    let first = f.post(&body).await;
    assert_eq!(first.status(), StatusCode::OK);
    let receipt: Value = first.json().await.unwrap();
    assert_eq!(receipt["operation_id"], body["operation_id"]);
    assert_eq!(receipt["record"]["created_via"], "management");
    assert_eq!(
        receipt["record"]["creator"]["principal_id"],
        f.owner.to_string()
    );
    let id = renoa_kernel::AgentId::from_uuid(
        Uuid::parse_str(receipt["record"]["id"].as_str().unwrap()).unwrap(),
    );
    let definition: Value = f
        .client
        .get(format!("{}/v1/host/agents/{id}", f.url))
        .header("cookie", &f.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(definition, receipt["record"]);
    assert_eq!(
        definition["operational"]["instructions"],
        body["instructions"]
    );
    assert_eq!(definition["operational"]["model"]["reasoning"], "high");
    f.restart().await;
    assert_eq!(f.post(&body).await.json::<Value>().await.unwrap(), receipt);
    assert_eq!(f.host.list_agents().await.unwrap().len(), 1);
    verify_execution(&f, id).await;
    let mut changed = body.clone();
    changed["name"] = "Other".into();
    assert_eq!(f.post(&changed).await.status(), StatusCode::CONFLICT);
    let snapshot: Value = f
        .client
        .get(format!("{}/v1/host", f.url))
        .header("cookie", &f.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot["agents"].as_array().unwrap().len(), 1);
    let discord: Value = f
        .client
        .get(format!("{}/v1/host/discord", f.url))
        .header("cookie", &f.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(discord, json!({"status":"setup_required"}));
    verify_discord_setup_is_refused_locally(&f).await;
    f.close().await;
}

/// Every refusal here is decided before Discord is contacted.
async fn verify_discord_setup_is_refused_locally(f: &Fixture) {
    let inspect = |origin: &'static str, token: &'static str| {
        f.client
            .post(format!("{}/v1/host/discord/inspection", f.url))
            .header("origin", origin)
            .header("cookie", &f.cookie)
            .json(&json!({"bot_token": token}))
            .send()
    };
    assert_eq!(
        inspect("https://wrong.example", "fixture.token")
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let refused = inspect(ORIGIN, "Bot fixture.token").await.unwrap();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let refused: Value = refused.json().await.unwrap();
    assert_eq!(refused["code"], "discord_rejected");
    let channels = f
        .client
        .get(format!("{}/v1/host/discord/channels", f.url))
        .header("cookie", &f.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(channels.status(), StatusCode::CONFLICT);
    let connect = f
        .client
        .post(format!("{}/v1/host/discord/connection", f.url))
        .header("origin", ORIGIN)
        .header("cookie", &f.cookie)
        .json(&json!({
            "operation_id": Uuid::new_v4(),
            "bot_token": "fixture.token",
            "guild_id": "10",
            "agent_id": Uuid::new_v4(),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(connect.status(), StatusCode::CONFLICT);
    let connect: Value = connect.json().await.unwrap();
    assert_eq!(
        connect["message"],
        "The selected agent does not exist on this Host"
    );
    assert!(
        !f.files
            .path()
            .join("home/credentials/discord.json")
            .exists()
    );
}

async fn verify_execution(f: &Fixture, id: renoa_kernel::AgentId) {
    let workspace = f.host.agent_workspace(id).await.unwrap();
    let session = f
        .host
        .ensure_agent_session(id, &workspace, Uuid::new_v4())
        .await
        .unwrap();
    let outcome = session
        .execute_turn_observed_with_cancellation(
            Uuid::new_v4(),
            vec![renoa_agent::ContentBlock::text("Run the proof")],
            TurnObservation::now().unwrap(),
            Arc::new(Quiet),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, renoa_local::LocalTurnOutcome::Completed { ref output, .. } if output == "Owner-created agent ran."),
        "{outcome:?}"
    );
    let request: Value =
        serde_json::from_slice(&std::fs::read(f.files.path().join("request.json")).unwrap())
            .unwrap();
    let tools: Vec<&str> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(tools.contains(&"read_file"));
    assert!(tools.contains(&"plugin_manage"));
    assert!(tools.contains(&"plugin_search"));
    assert!(tools.contains(&"tool_execute"));
    assert!(!tools.contains(&"bash"));
}

#[tokio::test]
async fn unauthorized_and_invalid_creation_leave_no_agents_or_documents() {
    let f = Fixture::new().await;
    let body = creation();
    assert_eq!(
        f.client
            .post(format!("{}/v1/host/agents", f.url))
            .header("origin", ORIGIN)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for origin in ["https://wrong.example", "null"] {
        assert_eq!(
            f.client
                .post(format!("{}/v1/host/agents", f.url))
                .header("origin", origin)
                .header("cookie", &f.cookie)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let wrong = pair(
        &f.client,
        &f.files.path().join("identity.sqlite3"),
        f.identity,
        PrincipalId::from_uuid(Uuid::new_v4()),
    )
    .await;
    assert_eq!(
        f.client
            .post(format!("{}/v1/host/agents", f.url))
            .header("origin", ORIGIN)
            .header("cookie", wrong)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    for (field, value) in [
        ("tools", json!(["unknown"])),
        (
            "model",
            json!({"provider":"xai","model":"absent","reasoning":"high"}),
        ),
        ("instructions", json!(" ")),
        ("creator", json!({"kind":"system","component":"forged"})),
    ] {
        let mut invalid = body.clone();
        invalid[field] = value;
        assert!(!f.post(&invalid).await.status().is_success());
    }
    assert!(f.host.list_agents().await.unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(f.files.path().join("home/agents"))
            .unwrap()
            .count(),
        0
    );
    f.close().await;
}

const MODEL: &str = r"
import {createHash} from 'node:crypto'; import fs from 'node:fs';
let input=''; for await (const part of process.stdin) input+=part;
const action=process.env.RENOA_MODEL_ACTION; const spec=process.env.RENOA_MODEL_SPEC;
if(action==='catalog') { process.stdout.write(JSON.stringify({ok:true,response:{models:[{id:'fixture-model',name:'Fixture',reasoning_levels:['high'],context_window_tokens:1000000,model_spec:{id:'fixture-model'}}]}})); }
else if(action==='describe') { process.stdout.write(JSON.stringify({ok:true,response:{context_window_tokens:1000000,max_output_tokens:8192,model_spec:spec,model_binding_id:createHash('sha256').update(spec).digest('hex'),reasoning_level:'high'}})); }
else if(action==='stream') { fs.writeFileSync(new URL('./request.json',import.meta.url),input); process.stdout.write(JSON.stringify({event:'completed',response:{content:[{type:'text',text:'Owner-created agent ran.'}],stop_reason:'stop',usage:{input:8,output:4,cache_read:0,cache_write:0},metadata:{api:'test',provider:process.env.RENOA_MODEL_PROVIDER,model:JSON.parse(spec).id}}})+'\n'); }
else process.exit(2);
";

#[tokio::test]
async fn the_owner_reads_and_edits_their_profile_for_agent_creation() {
    let f = Fixture::new().await;
    let users = f.files.path().join("home/users");
    let unsigned = f
        .client
        .get(format!("{}/v1/host/profile", f.url))
        .send()
        .await
        .unwrap();
    assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);

    let empty: Value = f.profile().await.json().await.unwrap();
    let empty_revision = empty["revision"].clone();
    assert_eq!(empty["content"], "");
    assert_eq!(empty_revision.as_str().unwrap().len(), 64);

    let guess = json!({"expected_revision": "0".repeat(64), "content": "Guessed.\n"});
    let stale = f.save_profile(ORIGIN, &guess).await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert!(!users.exists(), "a stale save must leave no profile behind");

    let first = json!({"expected_revision": empty_revision, "content": "Prefers mornings.\n"});
    let foreign = f.save_profile("https://elsewhere.example", &first).await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    assert!(!users.exists(), "a foreign origin must not save");

    let saved = f.save_profile(ORIGIN, &first).await;
    assert_eq!(saved.status(), StatusCode::OK);
    let saved: Value = saved.json().await.unwrap();
    assert_eq!(saved["content"], "Prefers mornings.\n");
    let profile = users.join(f.owner.as_uuid().to_string()).join("USER.md");
    assert_eq!(
        std::fs::read_to_string(&profile).unwrap(),
        "Prefers mornings.\n"
    );

    // An agent's later edit is what the next read shows, and a save based on
    // the older revision cannot overwrite it.
    std::fs::write(&profile, "Prefers mornings and tea.\n").unwrap();
    let current: Value = f.profile().await.json().await.unwrap();
    assert_eq!(current["content"], "Prefers mornings and tea.\n");
    let overwrite = json!({"expected_revision": saved["revision"], "content": "Overwrite.\n"});
    let outdated = f.save_profile(ORIGIN, &overwrite).await;
    assert_eq!(outdated.status(), StatusCode::CONFLICT);
    assert_eq!(
        std::fs::read_to_string(&profile).unwrap(),
        "Prefers mornings and tea.\n"
    );
    f.close().await;
}

#[tokio::test]
async fn a_profile_save_that_cannot_apply_is_refused_not_offered_as_a_retry() {
    let f = Fixture::new().await;
    let empty: Value = f.profile().await.json().await.unwrap();
    let malformed = json!({"expected_revision": "latest", "content": "Hi.\n"});
    let refused = f.save_profile(ORIGIN, &malformed).await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let elsewhere = tempfile::tempdir().unwrap();
    let users = f.files.path().join("home/users");
    std::fs::create_dir(&users).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), users.join(f.owner.as_uuid().to_string()))
        .unwrap();
    let linked = json!({"expected_revision": empty["revision"], "content": "Escaped.\n"});
    let refused = f.save_profile(ORIGIN, &linked).await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let refused: Value = refused.json().await.unwrap();
    assert_eq!(refused["code"], "invalid_profile");
    assert!(
        std::fs::read_dir(elsewhere.path())
            .unwrap()
            .next()
            .is_none(),
        "a linked profile must not receive the save"
    );
    f.close().await;
}
