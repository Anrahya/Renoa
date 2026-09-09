use std::{
    net::SocketAddr,
    time::{Duration, SystemTime},
};

use renoa_control::{BrowserSessions, Coordinator};
use renoa_kernel::AgentId;
use renoa_local::{
    ARCEE_PROFILE_ID, AgentProfile, AgentProfileId, AgentRecord, BotRecipe, BotRecord, LocalHost,
    LocalHostAdapters, LocalModelConfiguration, ModelProvider, RoutineMutation, RoutineRecord,
    RoutineSchedule, RoutineSpec,
};
use renoa_management::{ManagementApi, ManagementError};
use renoa_protocol::PrincipalId;
use reqwest::{Client, Response, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost";

struct Fixture {
    files: tempfile::TempDir,
    host: LocalHost,
    routine: RoutineRecord,
    owner: PrincipalId,
    identity_address: SocketAddr,
    identity_stop: CancellationToken,
    identity_task: tokio::task::JoinHandle<Result<(), renoa_control::ControlError>>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), ManagementError>>,
    url: String,
    cookie: String,
    client: Client,
}

impl Fixture {
    async fn new() -> Self {
        let files = tempfile::tempdir().expect("files");
        let root = files.path().join("host");
        let host = LocalHost::new(
            &root,
            LocalModelConfiguration::new(
                root.join("absent-model"),
                vec![ModelProvider::Xai],
                ModelProvider::Xai,
                "unused",
                root.join("absent-credentials"),
            ),
            vec![AgentProfile::new(ARCEE_PROFILE_ID, "Operator").expect("profile")],
            LocalHostAdapters::default(),
        )
        .expect("Host");
        let parent = AgentId::new();
        host.ensure_agent(AgentRecord {
            id: parent,
            profile: AgentProfileId::new(ARCEE_PROFILE_ID).expect("profile"),
            name: "Arcee".into(),
            created_by: None,
        })
        .await
        .expect("agent");
        let child = AgentId::new();
        host.ensure_bot(BotRecord {
            id: child,
            created_by: parent,
            recipe: BotRecipe {
                name: "News".into(),
                instructions: "Read news".into(),
                tools: ["write_file".to_owned()].into(),
                connections: std::collections::BTreeSet::new(),
            },
        })
        .await
        .expect("bot");
        let routine = host
            .manage_routine(
                parent,
                Uuid::new_v4(),
                RoutineMutation::Create {
                    spec: RoutineSpec {
                        agent_id: child,
                        name: "Brief".into(),
                        prompt: "Do not expose this standing prompt in the mutation receipt".into(),
                        schedule: RoutineSchedule::Interval { hours: 12 },
                        enabled: true,
                    },
                },
                0,
                CancellationToken::new(),
            )
            .await
            .expect("routine");
        let database = files.path().join("identity.sqlite");
        let control =
            Coordinator::open_with_passkeys(&database, "localhost", ORIGIN).expect("identity");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("identity listener");
        let identity_address = listener.local_addr().expect("address");
        let identity_stop = CancellationToken::new();
        let identity_task = tokio::spawn(control.serve(listener, identity_stop.clone()));
        let owner = PrincipalId::from_uuid(Uuid::new_v4());
        let client = Client::new();
        let cookie = pair(&client, &database, identity_address, owner).await;
        let api = ManagementApi::open(
            &root,
            host.host_id().await.expect("Host ID"),
            identity_address,
            owner,
            ORIGIN,
        )
        .expect("management");
        let (url, stop, task) = serve(api).await;
        Self {
            files,
            host,
            routine,
            owner,
            identity_address,
            identity_stop,
            identity_task,
            stop,
            task,
            url,
            cookie,
            client,
        }
    }
    fn endpoint(&self) -> String {
        format!("{}/v1/host/routines/{}/enabled", self.url, self.routine.id)
    }
    async fn change(&self, body: &Value) -> Response {
        self.client
            .post(self.endpoint())
            .header("origin", ORIGIN)
            .header("cookie", &self.cookie)
            .json(body)
            .send()
            .await
            .expect("request")
    }
    async fn finish(self) {
        self.stop.cancel();
        self.task.await.expect("task").expect("stop");
        self.identity_stop.cancel();
        self.identity_task
            .await
            .expect("identity task")
            .expect("identity stop");
    }
}

async fn serve(
    api: ManagementApi,
) -> (
    String,
    CancellationToken,
    tokio::task::JoinHandle<Result<(), ManagementError>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("management listener");
    let url = format!("http://{}", listener.local_addr().expect("address"));
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
    let sessions = BrowserSessions::open(database).expect("session issuer");
    let token = sessions
        .create_pairing(owner, SystemTime::now() + Duration::from_mins(30))
        .await
        .expect("code");
    let response = client
        .post(format!("http://{identity}/v1/identity/pair"))
        .header("origin", ORIGIN)
        .json(&json!({"pairingToken": token, "browserNonce": "78".repeat(32)}))
        .send()
        .await
        .expect("pair")
        .error_for_status()
        .expect("paired");
    response.headers()["set-cookie"]
        .to_str()
        .expect("cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned()
}

fn change(revision: i64, enabled: bool) -> Value {
    json!({"operation_id":Uuid::new_v4(), "expected_revision":revision, "enabled":enabled})
}

#[tokio::test]
async fn writes_require_the_paired_owner_exact_origin_and_a_strict_bounded_request() {
    let f = Fixture::new().await;
    let body = change(1, false);
    for origin in [
        None,
        Some("null"),
        Some("https://evil.example"),
        Some("http://localhost.evil.example"),
        Some("http://localhost:1234"),
    ] {
        let request = f
            .client
            .post(f.endpoint())
            .header("cookie", &f.cookie)
            .json(&body);
        let request = if let Some(origin) = origin {
            request.header("origin", origin)
        } else {
            request
        };
        assert_eq!(
            request.send().await.expect("origin check").status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.client
            .post(f.endpoint())
            .header("origin", ORIGIN)
            .header("origin", ORIGIN)
            .header("cookie", &f.cookie)
            .json(&body)
            .send()
            .await
            .expect("duplicate origin")
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.client
            .post(f.endpoint())
            .header("origin", ORIGIN)
            .header("x-renoa-principal", f.owner.to_string())
            .json(&body)
            .send()
            .await
            .expect("spoofed owner")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = pair(
        &f.client,
        &f.files.path().join("identity.sqlite"),
        f.identity_address,
        PrincipalId::from_uuid(Uuid::new_v4()),
    )
    .await;
    assert_eq!(
        f.client
            .post(f.endpoint())
            .header("origin", ORIGIN)
            .header("cookie", cookie)
            .json(&body)
            .send()
            .await
            .expect("wrong owner")
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut forged = body.clone();
    forged["actor_id"] = json!(f.owner);
    assert_eq!(
        f.change(&forged).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.change(&json!({"operation_id":"bad", "enabled":false}))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.change(&json!({"oversized":"a".repeat(5000)}))
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        f.host.routine(f.routine.id).await.expect("unchanged"),
        f.routine
    );
    BrowserSessions::open(f.files.path().join("identity.sqlite"))
        .expect("issuer")
        .revoke(f.owner)
        .await
        .expect("revoke");
    assert_eq!(f.change(&body).await.status(), StatusCode::UNAUTHORIZED);
    f.finish().await;
}

#[tokio::test]
async fn http_receipt_survives_restart_and_stale_edits_leave_the_shared_record_intact() {
    let mut f = Fixture::new().await;
    let pause = change(1, false);
    let response = f.change(&pause).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let receipt: Value = response.json().await.expect("receipt");
    assert_eq!(receipt["enabled"], false);
    assert_eq!(receipt["revision"], 2);
    assert!(receipt.get("spec").is_none());
    assert_eq!(
        f.host
            .routine(f.routine.id)
            .await
            .expect("same Host record")
            .revision,
        2
    );
    f.stop.cancel();
    f.task.await.expect("server task").expect("stop");
    let api = ManagementApi::open(
        &f.files.path().join("host"),
        f.host.host_id().await.expect("Host ID"),
        f.identity_address,
        f.owner,
        ORIGIN,
    )
    .expect("restart management");
    (f.url, f.stop, f.task) = serve(api).await;
    assert_eq!(
        f.change(&pause)
            .await
            .json::<Value>()
            .await
            .expect("replayed receipt"),
        receipt
    );
    assert_eq!(
        f.change(&change(1, true)).await.status(),
        StatusCode::CONFLICT
    );
    let response = f.change(&change(2, true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.expect("resume")["revision"],
        3
    );
    // A delayed duplicate returns historical acknowledgement, not latest state.
    assert_eq!(
        f.change(&pause)
            .await
            .json::<Value>()
            .await
            .expect("historical receipt"),
        receipt
    );
    let snapshot: Value = f
        .client
        .get(format!("{}/v1/host", f.url))
        .header("cookie", &f.cookie)
        .send()
        .await
        .expect("snapshot")
        .json()
        .await
        .expect("snapshot JSON");
    assert_eq!(snapshot["routines"][0]["enabled"], true);
    assert_eq!(snapshot["routines"][0]["revision"], 3);
    assert!(!f.files.path().join("host/absent-model").exists());
    assert!(!f.files.path().join("host/absent-credentials").exists());
    rusqlite::Connection::open(f.files.path().join("host/host.sqlite3"))
        .expect("catalog")
        .execute(
            "UPDATE host_identity SET host_id=?1",
            [Uuid::new_v4().to_string()],
        )
        .expect("replace Host");
    assert_eq!(
        f.change(&pause).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    f.finish().await;
}

#[tokio::test]
async fn expired_once_and_identity_outages_return_actionable_errors() {
    let mut f = Fixture::new().await;
    let mut spec = f.routine.spec.clone();
    spec.schedule = RoutineSchedule::Once {
        at: "1970-01-01T00:00:01Z".into(),
    };
    spec.enabled = false;
    f.host
        .manage_routine(
            spec.agent_id,
            Uuid::new_v4(),
            RoutineMutation::Update {
                id: f.routine.id,
                expected_revision: 1,
                spec,
            },
            2000,
            CancellationToken::new(),
        )
        .await
        .expect("paused expired schedule");
    let response = f.change(&change(2, true)).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response.json::<Value>().await.expect("validation")["code"],
        "invalid_schedule"
    );
    f.identity_stop.cancel();
    (&mut f.identity_task)
        .await
        .expect("identity task")
        .expect("stop");
    assert_eq!(
        f.change(&change(2, true)).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    f.stop.cancel();
    f.task.await.expect("management task").expect("stop");
}
