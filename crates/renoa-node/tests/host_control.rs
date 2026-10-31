//! The owner's control of the Host through the whole path: a signed-in
//! browser, the coordinator that relays, and the node that answers from the
//! Host's own records.

#[allow(
    dead_code,
    unused_imports,
    reason = "the shared node fixture exposes helpers used by the other node suites"
)]
mod support;

use std::{
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime},
};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_control::BrowserSessions;
use renoa_kernel::AgentId;
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, AutomationMutation,
    AutomationRecord, AutomationSchedule, AutomationSpec, LocalHost, LocalHostAdapters,
    LocalModelConfiguration, LocalTurnOutcome, ModelProvider, RenameAgent, Speaker,
    TurnObservation,
};
use renoa_node::{NodeError, RenoaNode};
use renoa_protocol::PrincipalId;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::{Value, json};
use support::TestSystem;
use tokio::{task::JoinHandle, time::timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost";

struct Quiet;

impl AgentEventSink for Quiet {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// A signed-in browser of the Control Room.
struct Browser {
    client: Client,
    http: String,
    cookie: String,
}

impl Browser {
    async fn sign_in(system: &TestSystem, principal: PrincipalId) -> Self {
        let client = Client::new();
        let http = system.http_url();
        let token = BrowserSessions::open(system.files.path().join("control.sqlite"))
            .expect("session issuer")
            .create_pairing(principal, SystemTime::now() + Duration::from_mins(30))
            .await
            .expect("pairing code");
        let response = client
            .post(format!("{http}/v1/identity/pair"))
            .header("origin", ORIGIN)
            .json(&json!({"pairingToken": token, "browserNonce": "67".repeat(32)}))
            .send()
            .await
            .expect("pair")
            .error_for_status()
            .expect("paired");
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .expect("cookie")
            .split(';')
            .next()
            .expect("cookie pair")
            .to_owned();
        Self {
            client,
            http,
            cookie,
        }
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.client
            .get(format!("{}{path}", self.http))
            .header("cookie", &self.cookie)
    }

    fn write(&self, method: Method, path: &str, origin: &str, body: &Value) -> RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.http))
            .header("origin", origin)
            .header("cookie", &self.cookie)
            .json(body)
    }

    async fn post(&self, path: &str, body: &Value) -> Response {
        self.write(Method::POST, path, ORIGIN, body)
            .send()
            .await
            .expect("POST")
    }

    async fn json_of(&self, path: &str) -> Value {
        self.get(path)
            .send()
            .await
            .expect("GET")
            .json()
            .await
            .expect("JSON")
    }
}

struct RunningNode {
    stop: CancellationToken,
    task: JoinHandle<Result<(), NodeError>>,
}

struct Fixture {
    system: TestSystem,
    host: Arc<LocalHost>,
    owner: PrincipalId,
    browser: Browser,
    node: Option<RunningNode>,
}

impl Fixture {
    /// The Host, the coordinator and a signed-in owner; no node yet.
    async fn new() -> Self {
        let system = TestSystem::start_with_browsers(ORIGIN).await;
        let root = system.files.path().join("home");
        let bridge = system.files.path().join("model.mjs");
        std::fs::write(&bridge, MODEL).expect("write model bridge");
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
        .expect("assemble the Host");
        std::fs::write(root.join("credentials/models.sqlite3"), "").expect("credential file");
        let owner = system.principal_id();
        let browser = Browser::sign_in(&system, owner).await;
        Self {
            system,
            host: Arc::new(host),
            owner,
            browser,
            node: None,
        }
    }

    /// A Host with its node connected and reachable from the browser.
    async fn serving() -> Self {
        let mut fixture = Self::new().await;
        fixture.start_node().await;
        fixture
    }

    fn root(&self) -> std::path::PathBuf {
        self.system.files.path().join("home")
    }

    async fn start_node(&mut self) {
        let node = RenoaNode::open(
            self.system.url.clone(),
            self.system.enroll_node().await,
            Arc::clone(&self.host),
        )
        .expect("open node");
        let stop = CancellationToken::new();
        let task = tokio::spawn(node.run(stop.clone()));
        self.node = Some(RunningNode { stop, task });
        self.wait_until_served().await;
    }

    async fn stop_node(&mut self) {
        let node = self.node.take().expect("a running node");
        node.stop.cancel();
        node.task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
    }

    /// Until the Host answers an observation. A node that is still
    /// connecting, or a connection that is going away, answers with a refusal.
    async fn wait_until_served(&self) {
        for _ in 0..1000 {
            let observed = self.browser.get("/v1/host").send().await.expect("GET");
            if observed.status() == StatusCode::OK {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the Host never became reachable");
    }

    async fn close(mut self) {
        if self.node.is_some() {
            self.stop_node().await;
        }
        self.system.stop().await;
    }
}

async fn within<T>(test: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(60), test)
        .await
        .expect("the test timed out")
}

fn creation() -> Value {
    json!({
        "operation_id": Uuid::new_v4(),
        "name": "Desk",
        "instructions": "Carry out the owner creation proof.",
        "purpose": "Keep my desk in order.",
        "tools": ["read_file"],
        "connections": [],
        "preset_id": null,
        "automation": null,
        "behavior": null,
        "documents": {"soul": true, "user": true},
        "model": {"provider": "xai", "model": "fixture-model", "reasoning": "high"},
    })
}

#[tokio::test]
async fn owner_creation_is_usable_and_identical_retry_survives_a_lost_reply_and_node_restart() {
    within(Box::pin(async {
        let mut f = Fixture::serving().await;
        let options = f.browser.json_of("/v1/host/agents/options").await;
        assert_eq!(options["native_tools"].as_array().expect("tools").len(), 6);
        assert_eq!(options["default_model"]["provider"], "xai");
        assert_eq!(options["models"][0]["reasoning_levels"], json!(["high"]));

        let mut body = creation();
        body["instructions"] = json!("Follow the owner instructions. ".repeat(300));
        let first = f.browser.post("/v1/host/agents", &body).await;
        assert_eq!(first.status(), StatusCode::OK);
        let receipt: Value = first.json().await.expect("receipt");
        assert_eq!(receipt["operation_id"], body["operation_id"]);
        assert_eq!(receipt["record"]["created_via"], "management");
        assert_eq!(
            receipt["record"]["creator"]["principal_id"],
            f.owner.to_string(),
            "the creator is the owner the coordinator signed in"
        );
        let id = AgentId::from_uuid(
            Uuid::parse_str(receipt["record"]["id"].as_str().expect("agent id")).expect("uuid"),
        );
        let definition = f.browser.json_of(&format!("/v1/host/agents/{id}")).await;
        assert_eq!(definition, receipt["record"]);
        assert_eq!(
            definition["operational"]["instructions"],
            body["instructions"]
        );
        assert_eq!(definition["operational"]["purpose"], body["purpose"]);

        // The reply may be lost with the node's connection; the same request
        // finds the same receipt, and nothing is created twice.
        f.stop_node().await;
        f.start_node().await;
        let replayed = f.browser.post("/v1/host/agents", &body).await;
        assert_eq!(replayed.json::<Value>().await.expect("replay"), receipt);
        assert_eq!(f.host.list_agents().await.expect("agents").len(), 1);
        verify_execution(&f, id).await;

        let mut changed = body.clone();
        changed["name"] = "Other".into();
        let conflict = f.browser.post("/v1/host/agents", &changed).await;
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let snapshot = f.browser.json_of("/v1/host").await;
        assert_eq!(snapshot["agents"].as_array().expect("agents").len(), 1);
        assert_eq!(
            f.browser.json_of("/v1/host/discord").await,
            json!({"status": "setup_required"})
        );
        verify_discord_setup_is_refused_locally(&f).await;
        f.close().await;
    }))
    .await;
}

/// Every refusal here is decided before Discord is contacted.
async fn verify_discord_setup_is_refused_locally(f: &Fixture) {
    let inspect = |origin: &str, token: &str| {
        f.browser
            .write(
                Method::POST,
                "/v1/host/discord/inspection",
                origin,
                &json!({"bot_token": token}),
            )
            .send()
    };
    assert_eq!(
        inspect("https://wrong.example", "fixture.token")
            .await
            .expect("foreign origin")
            .status(),
        StatusCode::FORBIDDEN
    );
    let refused = inspect(ORIGIN, "Bot fixture.token").await.expect("inspect");
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(
        refused.json::<Value>().await.expect("refusal")["code"],
        "discord_rejected"
    );
    let channels = f
        .browser
        .get("/v1/host/discord/channels")
        .send()
        .await
        .expect("channels");
    assert_eq!(channels.status(), StatusCode::CONFLICT);
    let connect = f
        .browser
        .post(
            "/v1/host/discord/connection",
            &json!({
                "operation_id": Uuid::new_v4(),
                "bot_token": "fixture.token",
                "guild_id": "10",
                "agent_id": Uuid::new_v4(),
            }),
        )
        .await;
    assert_eq!(connect.status(), StatusCode::CONFLICT);
    assert_eq!(
        connect.json::<Value>().await.expect("refusal")["message"],
        "The selected agent does not exist on this Host"
    );
    assert!(!f.root().join("credentials/discord.json").exists());
}

async fn verify_execution(f: &Fixture, id: AgentId) {
    let workspace = f.host.agent_workspace(id).await.expect("workspace");
    let session = f
        .host
        .ensure_agent_session(id, &workspace, Uuid::new_v4())
        .await
        .expect("session");
    let outcome = session
        .execute_turn_observed_with_cancellation(
            Uuid::new_v4(),
            vec![ContentBlock::text("Run the proof")],
            TurnObservation::now().expect("clock"),
            Arc::new(Quiet),
            CancellationToken::new(),
            Speaker::Unidentified,
        )
        .await
        .expect("turn");
    assert!(
        matches!(outcome, LocalTurnOutcome::Completed { ref output, .. } if output == "Owner-created agent ran."),
        "{outcome:?}"
    );
    let request: Value = serde_json::from_slice(
        &std::fs::read(f.system.files.path().join("request.json")).expect("request"),
    )
    .expect("request JSON");
    let tools: Vec<&str> = request["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect();
    assert!(tools.contains(&"read_file"));
    assert!(tools.contains(&"plugin_manage"));
    assert!(!tools.contains(&"bash"));
}

#[tokio::test]
async fn unauthorized_and_invalid_creation_leave_no_agents_or_documents() {
    within(Box::pin(async {
        let f = Fixture::serving().await;
        let body = creation();
        let anonymous = Client::new();
        let post = |client: &Client, origin: &str, cookie: Option<&str>| {
            let mut request = client
                .post(format!("{}/v1/host/agents", f.browser.http))
                .header("origin", origin)
                .json(&body);
            if let Some(cookie) = cookie {
                request = request.header("cookie", cookie);
            }
            request.send()
        };
        assert_eq!(
            post(&anonymous, ORIGIN, None).await.expect("anon").status(),
            StatusCode::UNAUTHORIZED
        );
        for origin in ["https://wrong.example", "null"] {
            let refused = post(&anonymous, origin, Some(&f.browser.cookie))
                .await
                .expect("origin");
            assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        }
        let stranger = Browser::sign_in(&f.system, PrincipalId::new()).await;
        let foreign = post(&anonymous, ORIGIN, Some(&stranger.cookie))
            .await
            .expect("foreign owner");
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);

        for (field, value) in [
            ("tools", json!(["unknown"])),
            (
                "model",
                json!({"provider": "xai", "model": "absent", "reasoning": "high"}),
            ),
            ("instructions", json!(" ")),
            ("purpose", json!(" ")),
            ("creator", json!({"kind": "system", "component": "forged"})),
        ] {
            let mut invalid = body.clone();
            invalid[field] = value;
            let refused = f.browser.post("/v1/host/agents", &invalid).await;
            assert!(!refused.status().is_success(), "{field}");
        }
        assert!(f.host.list_agents().await.expect("agents").is_empty());
        assert_eq!(
            std::fs::read_dir(f.root().join("agents"))
                .expect("agents directory")
                .count(),
            0
        );
        f.close().await;
    }))
    .await;
}

#[tokio::test]
async fn an_address_the_host_does_not_control_is_refused_by_the_host() {
    within(Box::pin(async {
        let f = Fixture::serving().await;
        let unknown = f.browser.get("/v1/host/nothing").send().await.expect("GET");
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            unknown.json::<Value>().await.expect("refusal")["code"],
            "not_found"
        );
        let wrong_method = f
            .browser
            .write(Method::PUT, "/v1/host/agents", ORIGIN, &json!({}))
            .send()
            .await
            .expect("PUT");
        assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
        let not_an_id = f
            .browser
            .get("/v1/host/agents/not-a-uuid")
            .send()
            .await
            .expect("GET");
        assert_eq!(not_an_id.status(), StatusCode::BAD_REQUEST);
        let absent = f
            .browser
            .get(&format!("/v1/host/agents/{}", Uuid::new_v4()))
            .send()
            .await
            .expect("GET");
        assert_eq!(absent.status(), StatusCode::NOT_FOUND);
        f.close().await;
    }))
    .await;
}

#[tokio::test]
async fn a_host_whose_node_is_offline_is_unavailable_and_returns_with_the_node() {
    within(Box::pin(async {
        let mut f = Fixture::serving().await;
        f.stop_node().await;
        // The coordinator learns of the departure asynchronously.
        let mut offline = None;
        for _ in 0..1000 {
            let answer = f.browser.get("/v1/host").send().await.expect("GET");
            if answer.status() == StatusCode::SERVICE_UNAVAILABLE {
                let body: Value = answer.json().await.expect("refusal");
                if body["code"] == "host_offline" {
                    offline = Some(body);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(offline.is_some(), "the Host is reported offline");
        f.start_node().await;
        assert_eq!(
            f.browser
                .get("/v1/host")
                .send()
                .await
                .expect("GET")
                .status(),
            StatusCode::OK
        );
        f.close().await;
    }))
    .await;
}

#[tokio::test]
async fn the_owner_reads_and_edits_their_profile_for_agent_creation() {
    within(Box::pin(async {
        let f = Fixture::serving().await;
        let users = f.root().join("users");
        let empty = f.browser.json_of("/v1/host/profile").await;
        assert_eq!(empty["content"], "");
        assert_eq!(empty["revision"].as_str().expect("revision").len(), 64);
        let save = |origin: &'static str, body: Value| {
            f.browser
                .write(Method::PUT, "/v1/host/profile", origin, &body)
                .send()
        };

        let guess = json!({"expected_revision": "0".repeat(64), "content": "Guessed.\n"});
        let stale = save(ORIGIN, guess).await.expect("stale");
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert!(!users.exists(), "a stale save must leave no profile behind");

        let first =
            json!({"expected_revision": empty["revision"], "content": "Prefers mornings.\n"});
        let foreign = save("https://elsewhere.example", first.clone())
            .await
            .expect("foreign");
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        assert!(!users.exists(), "a foreign origin must not save");

        let saved = save(ORIGIN, first).await.expect("save");
        assert_eq!(saved.status(), StatusCode::OK);
        let saved: Value = saved.json().await.expect("saved profile");
        assert_eq!(saved["content"], "Prefers mornings.\n");
        let profile = users.join(f.owner.as_uuid().to_string()).join("USER.md");
        assert_eq!(
            std::fs::read_to_string(&profile).expect("profile"),
            "Prefers mornings.\n"
        );

        // An agent's later edit is what the next read shows, and a save based
        // on the older revision cannot overwrite it.
        std::fs::write(&profile, "Prefers mornings and tea.\n").expect("agent edit");
        assert_eq!(
            f.browser.json_of("/v1/host/profile").await["content"],
            "Prefers mornings and tea.\n"
        );
        let overwrite = json!({"expected_revision": saved["revision"], "content": "Overwrite.\n"});
        let outdated = save(ORIGIN, overwrite).await.expect("outdated");
        assert_eq!(outdated.status(), StatusCode::CONFLICT);
        assert_eq!(
            std::fs::read_to_string(&profile).expect("profile"),
            "Prefers mornings and tea.\n"
        );
        f.close().await;
    }))
    .await;
}

const APP_INSTRUCTIONS: &str = include_str!("../../renoa-local/prompts/app-v1/system.md");
const APP_SOUL: &str = include_str!("../../renoa-local/prompts/app-v1/SOUL.md");

/// The prompt that makes the fixture model save a fact with `agent_documents`.
const REMEMBER: &str = "Remember that I am vegetarian.";
/// A person with no profile yet: an empty `USER.md` block carries the revision
/// the first save needs.
const EMPTY_USER: &str = "<user source=\"USER.md\" revision=\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\">\n\n</user>";

async fn owner_turn(
    f: &Fixture,
    id: AgentId,
    session: Uuid,
    text: &str,
) -> (LocalTurnOutcome, Value) {
    let workspace = f.host.agent_workspace(id).await.expect("workspace");
    let session = f
        .host
        .ensure_agent_session(id, &workspace, session)
        .await
        .expect("session");
    let outcome = session
        .execute_turn_observed_with_cancellation(
            Uuid::new_v4(),
            vec![ContentBlock::text(text)],
            TurnObservation::now().expect("clock"),
            Arc::new(Quiet),
            CancellationToken::new(),
            Speaker::Principal(f.owner.as_uuid()),
        )
        .await
        .expect("turn");
    let request = serde_json::from_slice(
        &std::fs::read(f.system.files.path().join("request.json")).expect("request"),
    )
    .expect("request JSON");
    (outcome, request)
}

/// Makes an agent through the owner's Host route, as the app does.
async fn make_agent(f: &Fixture, body: &Value) -> (AgentId, Value) {
    let created = f.browser.post("/v1/host/agents", body).await;
    assert_eq!(created.status(), StatusCode::OK);
    let record = created.json::<Value>().await.expect("receipt")["record"].clone();
    let id = AgentId::from_uuid(
        Uuid::parse_str(record["id"].as_str().expect("agent id")).expect("uuid"),
    );
    (id, record)
}

/// Every native file or shell tool the Host offers is absent from the request.
async fn assert_no_native_tools(f: &Fixture, request: &Value) {
    let tools: Vec<&str> = request["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect();
    let options = f.browser.json_of("/v1/host/agents/options").await;
    let native = options["native_tools"].as_array().expect("native tools");
    assert!(!native.is_empty());
    for name in native {
        let name = name.as_str().expect("native tool name");
        assert!(!tools.contains(&name), "{name} is a native tool");
    }
}

/// The request the Android app sends when the owner makes an agent.
#[tokio::test]
async fn an_app_agent_speaks_from_its_soul_and_keeps_the_owners_profile() {
    within(Box::pin(async {
        let f = Fixture::serving().await;
        let body = json!({
            "operation_id": Uuid::new_v4(),
            "preset_id": "renoa.app.v1",
            "name": "Kcal",
            "purpose": "Count my calories.",
        });
        let (id, record) = make_agent(&f, &body).await;
        assert_eq!(record["preset_id"], "renoa.app.v1");
        assert_eq!(
            record["operational"]["documents"],
            json!({"soul": true, "user": true})
        );
        assert_eq!(
            record["operational"]["behavior"]["workspace_instructions"],
            "off"
        );
        assert_eq!(record["tool_selection"]["tools"], json!([]));
        let soul = f.root().join("agents").join(id.to_string()).join("SOUL.md");
        assert_eq!(
            std::fs::read_to_string(soul).expect("published SOUL.md"),
            APP_SOUL
        );

        // Workspace rules stay out of an everyday helper's prompt.
        let workspace = f.host.agent_workspace(id).await.expect("workspace");
        std::fs::write(workspace.join("AGENTS.md"), "WORKSPACE_RULE_MARKER\n").expect("rules");

        // With no profile yet, the agent finds agent_documents through plugin
        // search and saves the owner's first fact with tool_execute.
        let session = Uuid::new_v4();
        let (saved, request) = owner_turn(&f, id, session, REMEMBER).await;
        assert!(
            matches!(saved, LocalTurnOutcome::Completed { ref output, .. } if output == "Saved."),
            "{saved:?}"
        );
        let first = request["system_prompt"].as_str().expect("system prompt");
        assert!(first.contains(EMPTY_USER), "{first}");
        let profile = f
            .root()
            .join("users")
            .join(f.owner.as_uuid().to_string())
            .join("USER.md");
        assert_eq!(
            std::fs::read_to_string(profile).expect("saved USER.md"),
            "Vegetarian.\n"
        );

        let (_, request) = owner_turn(&f, id, session, "What should I eat tonight?").await;
        let prompt = request["system_prompt"].as_str().expect("system prompt");
        let rest = prompt
            .strip_prefix(APP_INSTRUCTIONS.trim_end())
            .expect("the prompt opens with the app instructions");
        let at = |marker: &str| {
            rest.find(marker)
                .unwrap_or_else(|| panic!("{marker}: {prompt}"))
        };
        let order = [
            at("\n\n<identity>\nYour name is Kcal.\n</identity>"),
            at("\n<purpose>\nWhat you were made for:\nCount my calories.\n</purpose>"),
            at("<soul source=\"SOUL.md\""),
            at("<user source=\"USER.md\""),
        ];
        assert_eq!(order[0], 0, "the identity follows the instructions");
        assert!(
            order.is_sorted(),
            "instructions, identity, purpose, soul, user: {prompt}"
        );
        assert!(rest.contains(APP_SOUL.trim_end()), "{prompt}");
        assert!(rest.contains("\nVegetarian.\n</user>"), "{prompt}");
        assert!(!prompt.contains("WORKSPACE_RULE_MARKER"), "{prompt}");
        assert_no_native_tools(&f, &request).await;

        // A rename reaches the next turn of the same session.
        f.host
            .rename_agent(
                id,
                Uuid::new_v4(),
                RenameAgent {
                    id,
                    expected_name: "Kcal".to_owned(),
                    name: "Plate".to_owned(),
                },
                CancellationToken::new(),
            )
            .await
            .expect("rename");
        let (_, request) = owner_turn(&f, id, session, "And tomorrow?").await;
        let renamed = request["system_prompt"].as_str().expect("system prompt");
        assert!(
            renamed.contains("<identity>\nYour name is Plate.\n</identity>"),
            "{renamed}"
        );
        assert!(!renamed.contains("Your name is Kcal."), "{renamed}");
        f.close().await;
    }))
    .await;
}

/// The owner's agent was made from `renoa.general.v1` before the app preset.
/// Moving it while the node serves it keeps its session and history, and its
/// next turn runs on the app preset.
#[tokio::test]
async fn a_served_general_agent_moves_onto_the_app_preset_between_turns() {
    within(Box::pin(async {
        let f = Fixture::serving().await;
        let body = json!({
            "operation_id": Uuid::new_v4(),
            "preset_id": "renoa.general.v1",
            "name": "Renoa",
        });
        let (id, record) = make_agent(&f, &body).await;
        let session = Uuid::new_v4();
        let (_, before) = owner_turn(&f, id, session, "FIRST_TURN_MARKER").await;
        assert!(
            !before["system_prompt"]
                .as_str()
                .expect("prompt")
                .contains("<soul")
        );

        let moved = f
            .host
            .move_agent_to_preset(id, AgentPresetId::new("renoa.app.v1").expect("preset id"))
            .await
            .expect("move");
        assert!(moved.changed);
        let soul = f.root().join("agents").join(id.to_string()).join("SOUL.md");
        assert_eq!(std::fs::read_to_string(soul).expect("SOUL.md"), APP_SOUL);

        let (_, after) = owner_turn(&f, id, session, "What should I eat tonight?").await;
        let prompt = after["system_prompt"].as_str().expect("prompt");
        assert!(prompt.starts_with(APP_INSTRUCTIONS.trim_end()), "{prompt}");
        assert!(prompt.contains(APP_SOUL.trim_end()), "{prompt}");
        assert!(prompt.contains(EMPTY_USER), "{prompt}");
        assert!(
            after["messages"].to_string().contains("FIRST_TURN_MARKER"),
            "the session keeps its history"
        );
        let definition = f.browser.json_of(&format!("/v1/host/agents/{id}")).await;
        assert_eq!(definition["name"], "Renoa");
        assert_eq!(definition["preset_id"], "renoa.app.v1");
        assert_eq!(definition["tool_selection"], record["tool_selection"]);
        f.close().await;
    }))
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_profile_save_that_cannot_apply_is_refused_not_offered_as_a_retry() {
    within(Box::pin(async {
        let f = Fixture::serving().await;
        let empty = f.browser.json_of("/v1/host/profile").await;
        let put = |body: Value| {
            f.browser
                .write(Method::PUT, "/v1/host/profile", ORIGIN, &body)
                .send()
        };
        let malformed = json!({"expected_revision": "latest", "content": "Hi.\n"});
        let refused = put(malformed).await.expect("malformed");
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let elsewhere = tempfile::tempdir().expect("directory");
        let users = f.root().join("users");
        std::fs::create_dir(&users).expect("users");
        std::os::unix::fs::symlink(elsewhere.path(), users.join(f.owner.as_uuid().to_string()))
            .expect("link");
        let linked = json!({"expected_revision": empty["revision"], "content": "Escaped.\n"});
        let refused = put(linked).await.expect("linked");
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            refused.json::<Value>().await.expect("refusal")["code"],
            "invalid_profile"
        );
        assert!(
            std::fs::read_dir(elsewhere.path())
                .expect("directory")
                .next()
                .is_none(),
            "a linked profile must not receive the save"
        );
        f.close().await;
    }))
    .await;
}

/// Provisions the operator and one specialist, then gives the specialist a
/// standing automation the owner can pause.
async fn seed_automation(host: &LocalHost) -> AutomationRecord {
    let provision = |preset: &str, name: &str| {
        AgentCreateRequest::from_preset(
            Uuid::new_v4(),
            AgentPresetId::new(preset).expect("preset id"),
            name,
        )
    };
    let creator = || AgentCreator::System {
        component: "host-control-test".to_owned(),
    };
    let parent = host
        .create_agent(
            creator(),
            AgentCreationOrigin::Provisioning,
            provision("renoa.personal.arcee.v3", "Arcee"),
            CancellationToken::new(),
        )
        .await
        .expect("operator")
        .id;
    let child = host
        .create_agent(
            creator(),
            AgentCreationOrigin::Provisioning,
            provision("renoa.general.v1", "News")
                .with_instructions("Read news")
                .with_tools(["write_file".to_owned()]),
            CancellationToken::new(),
        )
        .await
        .expect("specialist")
        .id;
    host.manage_automation(
        parent,
        Uuid::new_v4(),
        AutomationMutation::Create {
            spec: AutomationSpec {
                agent_id: child,
                name: "Brief".into(),
                prompt: "Do not expose this standing prompt in the mutation receipt".into(),
                schedule: AutomationSchedule::Cron {
                    expression: "0 */12 * * *".to_owned(),
                    timezone: "UTC".to_owned(),
                },
                enabled: true,
            },
        },
        0,
        CancellationToken::new(),
    )
    .await
    .expect("automation")
}

fn pause(revision: i64, enabled: bool) -> Value {
    json!({"operation_id": Uuid::new_v4(), "expected_revision": revision, "enabled": enabled})
}

fn endpoint(automation: &AutomationRecord) -> String {
    format!("/v1/host/automations/{}/enabled", automation.id)
}

#[tokio::test]
async fn automation_writes_take_a_strict_bounded_request() {
    within(Box::pin(async {
        let mut f = Fixture::new().await;
        let automation = seed_automation(&f.host).await;
        f.start_node().await;
        let path = endpoint(&automation);
        let mut forged = pause(1, false);
        forged["actor_id"] = json!(f.owner);
        for (body, status) in [
            (forged, StatusCode::UNPROCESSABLE_ENTITY),
            (
                json!({"operation_id": "bad", "enabled": false}),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                json!({"oversized": "a".repeat(5000)}),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
        ] {
            assert_eq!(f.browser.post(&path, &body).await.status(), status);
        }
        let unknown = f
            .browser
            .post(
                &format!("/v1/host/automations/{}/enabled", Uuid::new_v4()),
                &pause(1, false),
            )
            .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            f.host
                .automation(automation.spec.agent_id, automation.id)
                .await
                .expect("unchanged"),
            automation
        );
        f.close().await;
    }))
    .await;
}

#[tokio::test]
async fn a_receipt_survives_a_node_restart_and_stale_edits_leave_the_record_intact() {
    within(Box::pin(async {
        let mut f = Fixture::new().await;
        let automation = seed_automation(&f.host).await;
        f.start_node().await;
        let path = endpoint(&automation);

        let paused = pause(1, false);
        let response = f.browser.post(&path, &paused).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let receipt: Value = response.json().await.expect("receipt");
        assert_eq!(receipt["enabled"], false);
        assert_eq!(receipt["revision"], 2);
        assert!(
            receipt.get("spec").is_none(),
            "the receipt keeps the prompt out"
        );
        assert_eq!(
            f.host
                .automation(automation.spec.agent_id, automation.id)
                .await
                .expect("same Host record")
                .revision,
            2
        );

        f.stop_node().await;
        f.start_node().await;
        let replayed = f.browser.post(&path, &paused).await;
        assert_eq!(replayed.json::<Value>().await.expect("replay"), receipt);
        assert_eq!(
            f.browser.post(&path, &pause(1, true)).await.status(),
            StatusCode::CONFLICT
        );
        let resumed = f.browser.post(&path, &pause(2, true)).await;
        assert_eq!(resumed.status(), StatusCode::OK);
        assert_eq!(
            resumed.json::<Value>().await.expect("resume")["revision"],
            3
        );
        // A delayed duplicate returns the historical acknowledgement.
        let late = f.browser.post(&path, &paused).await;
        assert_eq!(late.json::<Value>().await.expect("historical"), receipt);
        let snapshot = f.browser.json_of("/v1/host").await;
        assert_eq!(snapshot["automations"][0]["enabled"], true);
        assert_eq!(snapshot["automations"][0]["revision"], 3);

        // A Host replaced under a running node is not served as the old one.
        rusqlite::Connection::open(f.root().join("state/host.sqlite3"))
            .expect("catalog")
            .execute(
                "UPDATE host_identity SET host_id=?1",
                [Uuid::new_v4().to_string()],
            )
            .expect("replace Host");
        assert_eq!(
            f.browser.post(&path, &paused).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let profile = f.browser.get("/v1/host/profile").send().await.expect("GET");
        assert_eq!(profile.status(), StatusCode::SERVICE_UNAVAILABLE);
        f.close().await;
    }))
    .await;
}

#[tokio::test]
async fn an_expired_once_schedule_cannot_be_resumed() {
    within(Box::pin(async {
        let mut f = Fixture::new().await;
        let automation = seed_automation(&f.host).await;
        let mut spec = automation.spec.clone();
        spec.schedule = AutomationSchedule::Once {
            at: "1970-01-01T00:00:01Z".into(),
        };
        spec.enabled = false;
        f.host
            .manage_automation(
                spec.agent_id,
                Uuid::new_v4(),
                AutomationMutation::Update {
                    id: automation.id,
                    expected_revision: 1,
                    spec,
                },
                2000,
                CancellationToken::new(),
            )
            .await
            .expect("paused expired schedule");
        f.start_node().await;
        let response = f
            .browser
            .post(&endpoint(&automation), &pause(2, true))
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response.json::<Value>().await.expect("validation")["code"],
            "invalid_schedule"
        );
        f.close().await;
    }))
    .await;
}

#[tokio::test]
async fn the_owner_names_the_main_agent_from_the_revision_it_read() {
    within(Box::pin(async {
        let mut f = Fixture::new().await;
        let second = json!(seed_automation(&f.host).await.spec.agent_id);
        f.start_node().await;
        let observed = f.browser.json_of("/v1/host").await;
        let first = observed["main_agent"]["main_agent_id"].clone();
        assert_eq!(observed["main_agent"]["revision"], 1);
        assert_ne!(first, second, "the first agent made is the main agent");

        let change = json!({"operation_id": Uuid::new_v4(), "expected_revision": 1, "agent_id": second});
        let response = f.browser.post("/v1/host/main-agent", &change).await;
        assert_eq!(response.status(), StatusCode::OK);
        let named: Value = response.json().await.expect("main agent");
        assert_eq!(named, json!({"main_agent_id": second, "revision": 2}));
        assert_eq!(f.browser.json_of("/v1/host").await["main_agent"], named);
        let retried = f.browser.post("/v1/host/main-agent", &change).await;
        assert_eq!(retried.status(), StatusCode::OK);
        assert_eq!(retried.json::<Value>().await.expect("receipt"), named);

        let mut another_body = change.clone();
        another_body["agent_id"] = first.clone();
        let mut forged = change.clone();
        forged["principal_id"] = json!(Uuid::new_v4());
        for (body, status, code) in [
            (another_body, StatusCode::CONFLICT, "operation_conflict"),
            (
                json!({"operation_id": Uuid::new_v4(), "expected_revision": 1, "agent_id": first}),
                StatusCode::CONFLICT,
                "revision_conflict",
            ),
            (
                json!({"operation_id": Uuid::new_v4(), "expected_revision": 2, "agent_id": Uuid::new_v4()}),
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (forged, StatusCode::UNPROCESSABLE_ENTITY, "invalid_request"),
        ] {
            let refused = f.browser.post("/v1/host/main-agent", &body).await;
            assert_eq!(refused.status(), status, "{body}");
            assert_eq!(refused.json::<Value>().await.expect("refusal")["code"], code);
        }
        assert_eq!(f.browser.json_of("/v1/host").await["main_agent"], named);
        f.close().await;
    }))
    .await;
}

const MODEL: &str = r"
import {createHash} from 'node:crypto'; import fs from 'node:fs';
let input=''; for await (const part of process.stdin) input+=part;
const action=process.env.RENOA_MODEL_ACTION; const spec=process.env.RENOA_MODEL_SPEC;
if(action==='catalog') { process.stdout.write(JSON.stringify({ok:true,response:{models:[{id:'fixture-model',name:'Fixture',reasoning_levels:['high'],context_window_tokens:1000000,model_spec:{id:'fixture-model'}}]}})); }
else if(action==='describe') { process.stdout.write(JSON.stringify({ok:true,response:{context_window_tokens:1000000,max_output_tokens:8192,model_spec:spec,model_binding_id:createHash('sha256').update(spec).digest('hex'),reasoning_level:'high'}})); }
else if(action==='stream') {
 fs.writeFileSync(new URL('./request.json',import.meta.url),input);
 const request=JSON.parse(input);
 const lastUser=request.messages.findLastIndex(message=>message.role==='user');
 const results=request.messages.slice(lastUser+1).filter(message=>message.role==='tool').map(message=>message.result);
 const reply=(content,stop_reason)=>process.stdout.write(JSON.stringify({event:'completed',response:{content,stop_reason,usage:{input:8,output:4,cache_read:0,cache_write:0},metadata:{api:'test',provider:process.env.RENOA_MODEL_PROVIDER,model:JSON.parse(spec).id}}})+'\n');
 const call=(id,name,args)=>reply([{type:'tool_call',id,name,arguments:args}],'tool_use');
 if(request.messages[lastUser].content[0].text!=='Remember that I am vegetarian.') reply([{type:'text',text:'Owner-created agent ran.'}],'stop');
 else if(results.length===0) call('find','plugin_search',{query:'agent_documents'});
 else {
  const last=results.at(-1); if(last.is_error) throw Error(JSON.stringify(last));
  const value=last.name==='plugin_search' ? JSON.parse(last.content[0].text) : null;
  const match=value?.tool_matches?.find(tool=>tool.name==='agent_documents') ?? value;
  if(!value) reply([{type:'text',text:'Saved.'}],'stop');
  else if(!match.input_schema) call('schema','plugin_search',{reference:match.reference});
  else {
   const revision=request.system_prompt.match(/<user source=.USER\.md. revision=.([a-f0-9]{64})/)[1];
   call('save','tool_execute',{reference:match.reference,arguments:{document:'user',expected_revision:revision,content:'Vegetarian.\n'}});
  }
 }
}
else process.exit(2);
";
