use std::{collections::BTreeSet, path::Path};

use renoa_kernel::{AgentId, Command, CommandId, Kernel, SessionId};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    AgentProfileId, AgentRecord, BotRecipe, BotRecord, LocalHost, LocalHostAdapters,
    LocalModelConfiguration, ModelProvider, ReasoningLevel, RoutineMutation, RoutineSchedule,
    RoutineSpec, alpha_profile, host_storage::create_session_storage, selection::RuntimeSelection,
};

fn host(root: &Path) -> LocalHost {
    LocalHost::new(
        root,
        LocalModelConfiguration::new(
            root.join("model-does-not-exist"),
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "unavailable-model",
            root.join("credentials-do-not-exist"),
        ),
        vec![alpha_profile()],
        LocalHostAdapters::default(),
    )
    .expect("Host without model startup")
}

async fn agent(host: &LocalHost) -> AgentRecord {
    host.ensure_agent(AgentRecord {
        id: AgentId::new(),
        profile: AgentProfileId::new(crate::ALPHA_PROFILE_ID).expect("profile"),
        name: "Operator".to_owned(),
        created_by: None,
    })
    .await
    .expect("agent")
}

fn session(root: &Path, agent: &AgentRecord) -> (Uuid, Kernel) {
    let id = Uuid::new_v4();
    create_session_storage(
        &root.join("sessions"),
        agent.profile.clone(),
        agent.id,
        SessionId::from_uuid(id),
        root.to_owned(),
        &RuntimeSelection {
            provider: ModelProvider::Xai,
            model: "unavailable".to_owned(),
            reasoning: ReasoningLevel::High,
        },
    )
    .expect("published session");
    let owner = Kernel::open(
        root.join("sessions")
            .join(id.to_string())
            .join("kernel.sqlite3"),
    )
    .expect("execution owner");
    (id, owner)
}

#[tokio::test]
async fn observes_owned_sessions_without_loading_models_or_repairing_runtime_logs() {
    let root = tempfile::tempdir().expect("root");
    let host = host(root.path());
    let agent = agent(&host).await;
    let (id, owner) = session(root.path(), &agent);
    let runtime_file = root
        .path()
        .join("sessions")
        .join(id.to_string())
        .join("runtime.jsonl");
    std::fs::write(&runtime_file, "torn record, must remain untouched").expect("torn metadata");
    owner
        .submit(
            SessionId::from_uuid(id),
            Command::new(
                CommandId::new(),
                serde_json::json!({"prompt":"PRIVATE PAYLOAD"}),
            ),
        )
        .expect("admit");
    let observer = HostObserver::open(root.path()).expect("observer");
    let snapshot = observer.snapshot().await.expect("snapshot");
    assert_eq!(snapshot.host_id, host.host_id().await.expect("identity"));
    assert_eq!(snapshot.agents.len(), 1);
    assert!(matches!(
        snapshot.sessions[0].state,
        ObservedSessionState::Available {
            queued_operations: 1,
            ..
        }
    ));
    assert!(
        !serde_json::to_string(&snapshot)
            .expect("json")
            .contains("PRIVATE PAYLOAD")
    );
    assert_eq!(
        std::fs::read_to_string(runtime_file).expect("metadata"),
        "torn record, must remain untouched"
    );
    drop(owner);
    let restarted = HostObserver::open(root.path()).expect("observer restart");
    assert_eq!(
        serde_json::to_value(snapshot).expect("before"),
        serde_json::to_value(restarted.snapshot().await.expect("after")).expect("json")
    );
}

#[tokio::test]
async fn projects_shared_inventory_and_routine_mutations_without_copying_secrets() {
    let root = tempfile::tempdir().expect("root");
    let host = host(root.path());
    let creator = agent(&host).await;
    let db =
        catalog::open_verified(&root.path().join(catalog::HOST_DATABASE)).expect("fixture catalog");
    db.execute_batch("INSERT INTO mcp_integrations(integration_id,kind,endpoint,request_headers_json)
        VALUES('x','direct_streamable_http','https://example.com/mcp','{\"Authorization\":\"SECRET HEADER\"}');
        INSERT INTO mcp_connections(connection_id,integration_id,auth_kind) VALUES('x-api','x','none');
        INSERT INTO mcp_catalogs(connection_id,endpoint,request_headers_json,protocol_version,adapter_revision,catalog_digest)
        VALUES('x-api','https://example.com/mcp','{}','2025-03-26','fixture',printf('%064d',0));").expect("record connection without network");
    let bot = host
        .ensure_bot(BotRecord {
            id: AgentId::new(),
            created_by: creator.id,
            recipe: BotRecipe {
                name: "X Desk".to_owned(),
                instructions: "PRIVATE INSTRUCTIONS".to_owned(),
                tools: BTreeSet::from(["read_file".to_owned()]),
                connections: BTreeSet::from(["x-api".to_owned()]),
            },
        })
        .await
        .expect("specialist");
    let id = Uuid::new_v4();
    let routine = host
        .manage_routine(
            bot.id,
            id,
            RoutineMutation::Create {
                spec: RoutineSpec {
                    agent_id: bot.id,
                    name: "Digest".to_owned(),
                    prompt: "PRIVATE ROUTINE PROMPT".to_owned(),
                    schedule: RoutineSchedule::Daily {
                        hour: 9,
                        minute: 0,
                        timezone: "Asia/Kolkata".to_owned(),
                    },
                    enabled: true,
                },
            },
            1_789_000_000_000,
            CancellationToken::new(),
        )
        .await
        .expect("routine");
    let observer = HostObserver::open(root.path()).expect("observer");
    let snapshot = observer.snapshot().await.expect("snapshot");
    assert_eq!(snapshot.routines.len(), 1);
    assert_eq!(snapshot.routines[0].revision, routine.revision);
    assert_eq!(
        snapshot.connections[0].selected_by_profiles,
        vec![format!("renoa.bot.{}", bot.id)]
    );
    assert!(snapshot.connections[0].catalog_available);
    let encoded = serde_json::to_string(&snapshot).expect("json");
    for secret in [
        "SECRET HEADER",
        "PRIVATE INSTRUCTIONS",
        "PRIVATE ROUTINE PROMPT",
        "https://example.com",
    ] {
        assert!(!encoded.contains(secret));
    }
    host.manage_routine(
        bot.id,
        Uuid::new_v4(),
        RoutineMutation::Delete {
            id,
            expected_revision: 1,
        },
        1_789_000_000_001,
        CancellationToken::new(),
    )
    .await
    .expect("delete");
    assert!(
        observer
            .snapshot()
            .await
            .expect("after delete")
            .routines
            .is_empty()
    );
}

#[tokio::test]
async fn isolates_corrupt_sessions_and_does_not_import_legacy_agents() {
    let root = tempfile::tempdir().expect("root");
    let host = host(root.path());
    let agent = agent(&host).await;
    let (id, _owner) = session(root.path(), &agent);
    let db = catalog::open_verified(&root.path().join(catalog::HOST_DATABASE)).expect("catalog");
    db.execute("DELETE FROM host_agents", [])
        .expect("legacy session");
    let bad = root
        .path()
        .join("sessions")
        .join(Uuid::new_v4().to_string());
    std::fs::create_dir(&bad).expect("bad session");
    std::fs::write(bad.join("session.json"), "invalid").expect("corrupt metadata");
    let snapshot = HostObserver::open(root.path())
        .expect("observer")
        .snapshot()
        .await
        .expect("partial snapshot");
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.sessions.len(), 2);
    assert!(matches!(
        snapshot
            .sessions
            .iter()
            .find(|s| s.id == id)
            .expect("valid session")
            .state,
        ObservedSessionState::Available { .. }
    ));
    assert!(
        snapshot
            .sessions
            .iter()
            .any(|s| matches!(s.state, ObservedSessionState::Unavailable { .. }))
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM host_agents", [], |r| r
            .get::<_, i64>(0))
            .expect("count"),
        0
    );
}

#[tokio::test]
async fn pins_host_identity_and_never_initializes_an_empty_root() {
    let root = tempfile::tempdir().expect("root");
    assert!(HostObserver::open(root.path()).is_err());
    assert!(!root.path().join(catalog::HOST_DATABASE).exists());
    let _host = host(root.path());
    let observer = HostObserver::open(root.path()).expect("observer");
    let db = catalog::open_verified(&root.path().join(catalog::HOST_DATABASE)).expect("catalog");
    db.execute(
        "UPDATE host_identity SET host_id=?1",
        [Uuid::new_v4().to_string()],
    )
    .expect("replace identity");
    assert!(observer.snapshot().await.is_err());
}

#[tokio::test]
async fn review_inventory_distinguishes_queued_and_incomplete_without_hydrating_context() {
    use crate::{GitHubReviewCommand, GitHubReviewPolicy, GitHubReviewTrigger};
    let root = tempfile::tempdir().expect("root");
    let host = host(root.path());
    let agent = agent(&host).await;
    let reviewer = host
        .ensure_bot(BotRecord {
            id: AgentId::new(),
            created_by: agent.id,
            recipe: BotRecipe {
                name: "Soundwave".to_owned(),
                instructions: "Review code".to_owned(),
                tools: BTreeSet::new(),
                connections: BTreeSet::new(),
            },
        })
        .await
        .expect("reviewer");
    host.manage_github_review(
        GitHubReviewCommand::SetRepository {
            operation_id: Uuid::new_v4(),
            expected_revision: None,
            policy: GitHubReviewPolicy {
                repository_id: 1,
                installation_id: 2,
                full_name: "owner/repo".to_owned(),
                agent_id: reviewer.id,
                enabled: true,
                triggers: BTreeSet::from([GitHubReviewTrigger::Opened]),
                skip_drafts: true,
            },
        },
        1_789_000_000_000,
        CancellationToken::new(),
    )
    .await
    .expect("repository");
    let id = Uuid::new_v4();
    host.manage_github_review(
        GitHubReviewCommand::Request {
            operation_id: id,
            repository_id: 1,
            pull_number: 21,
            reported_base_sha: "a".repeat(40),
            reported_head_sha: "b".repeat(40),
        },
        1_789_000_000_000,
        CancellationToken::new(),
    )
    .await
    .expect("request");
    let observer = HostObserver::open(root.path()).expect("observer");
    let view = observer.snapshot().await.expect("queued view");
    assert_eq!(view.reviews.len(), 1);
    assert!(matches!(
        view.reviews[0].state,
        super::ObservedReviewState::Queued
    ));
    record_incomplete_review(root.path(), id);
    let view = observer.snapshot().await.expect("terminal view");
    assert!(matches!(
        view.reviews[0].state,
        super::ObservedReviewState::Incomplete
    ));
    assert_eq!(view.reviews[0].repository, "owner/repo");
    assert!(view.reviews[0].reviewed_head_sha.is_none());
    assert!(
        !serde_json::to_string(&view)
            .expect("json")
            .contains("PRIVATE PROVIDER DIAGNOSTICS")
    );
    let detail = observer
        .review_detail(id)
        .await
        .expect("selected detail")
        .expect("known review");
    assert_eq!(
        detail.reason.as_deref(),
        Some("PRIVATE PROVIDER DIAGNOSTICS")
    );
    assert!(detail.report.is_none());
    assert!(
        observer
            .review_detail(Uuid::new_v4())
            .await
            .expect("unknown detail")
            .is_none()
    );
}

fn record_incomplete_review(root: &std::path::Path, id: Uuid) {
    let db = catalog::open_verified(&root.join(catalog::HOST_DATABASE)).expect("catalog");
    let record = crate::GitHubReviewRun::Finished {
        request_id: id,
        snapshot: None,
        outcome: crate::GitHubReviewOutcome::Incomplete {
            reason: "PRIVATE PROVIDER DIAGNOSTICS".to_owned(),
        },
    };
    db.execute(
        "INSERT INTO host_review_runs(request_id,terminal,record_json) VALUES(?1,1,?2)",
        rusqlite::params![
            id.to_string(),
            serde_json::to_string(&record).expect("fixture outcome")
        ],
    )
    .expect("terminal record");
}
