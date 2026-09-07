use super::*;
use crate::{
    AgentProfile, AgentProfileId, AgentRecord, BotRecipe, BotRecord, ModelProvider,
    host::HostInitialization,
};
use ring::hmac;
use std::{fs, path::Path};

mod admission;
mod executor;
pub(super) mod source_tool;

const SECRET: &[u8] = b"deterministic webhook boundary secret";

fn host(root: &Path) -> LocalHost {
    LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::Xai],
        initial_provider: ModelProvider::Xai,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("auth.sqlite"),
        mcp_adapter: None,
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: None,
        oauth_relay: None,
        profiles: vec![AgentProfile::new(crate::ARCEE_PROFILE_ID, "Operator").expect("profile")],
    })
    .expect("Host")
}

async fn fixture() -> (tempfile::TempDir, LocalHost, GitHubReviewPolicy) {
    let directory = tempfile::tempdir().expect("directory");
    fs::write(
        directory.path().join("model.mjs"),
        "throw new Error('admission must not call a model');",
    )
    .expect("model boundary");
    fs::write(directory.path().join("auth.sqlite"), "").expect("auth boundary");
    let host = host(directory.path());
    let operator = AgentId::new();
    host.ensure_agent(AgentRecord {
        id: operator,
        profile: AgentProfileId::new(crate::ARCEE_PROFILE_ID).expect("profile"),
        name: "Arcee".to_owned(),
        created_by: None,
    })
    .await
    .expect("operator");
    let reviewer = AgentId::new();
    host.ensure_bot(BotRecord {
        id: reviewer,
        created_by: operator,
        recipe: BotRecipe {
            name: "Review Desk".to_owned(),
            instructions: "Investigate code defects".to_owned(),
            tools: ["read_file".to_owned(), "grep".to_owned()].into(),
            connections: BTreeSet::new(),
        },
    })
    .await
    .expect("specialist");
    let policy = GitHubReviewPolicy {
        repository_id: 42,
        installation_id: 7,
        full_name: "owner/repository".to_owned(),
        agent_id: reviewer,
        enabled: true,
        triggers: [
            GitHubReviewTrigger::Opened,
            GitHubReviewTrigger::ReadyForReview,
            GitHubReviewTrigger::Synchronize,
        ]
        .into(),
        skip_drafts: true,
    };
    set(&host, policy.clone(), None).await;
    (directory, host, policy)
}

async fn set(host: &LocalHost, policy: GitHubReviewPolicy, revision: Option<i64>) {
    host.manage_github_review(
        GitHubReviewCommand::SetRepository {
            operation_id: Uuid::new_v4(),
            expected_revision: revision,
            policy,
        },
        100,
        CancellationToken::new(),
    )
    .await
    .expect("set policy");
}

fn payload(head: char) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({ "action":"opened", "installation":{"id":7},
        "repository":{"id":42}, "pull_request":{"number":14,"draft":false,"state":"open",
        "base":{"sha":"a".repeat(40)},"head":{"sha":head.to_string().repeat(40)}} }),
    )
    .expect("payload")
}

fn signature(body: &[u8]) -> String {
    use std::fmt::Write as _;
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, SECRET), body);
    let mut result = "sha256=".to_owned();
    for byte in tag.as_ref() {
        write!(result, "{byte:02x}").expect("hex");
    }
    result
}

async fn deliver(
    host: &LocalHost,
    id: Uuid,
    body: &[u8],
) -> Result<GitHubReviewAdmission, LocalHostError> {
    host.admit_github_review_webhook(
        GitHubReviewWebhook {
            delivery_id: id,
            event: "pull_request",
            signature: &signature(body),
            body,
        },
        SECRET,
        500,
        CancellationToken::new(),
    )
    .await
}

async fn requests(host: &LocalHost, after: i64) -> Vec<GitHubReviewRequest> {
    let GitHubReviewReply::Requests { records } = host
        .manage_github_review(
            GitHubReviewCommand::Requests { after },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("list requests")
    else {
        panic!("request page expected")
    };
    records
}

#[tokio::test]
async fn authenticated_admission_survives_restart_and_deduplicates_delivery_and_logical_work() {
    let (directory, host, _) = fixture().await;
    let identity = host.host_id().await.expect("identity");
    let id = Uuid::new_v4();
    let body = payload('b');
    let first = deliver(&host, id, &body).await.expect("admit");
    assert!(matches!(first, GitHubReviewAdmission::Queued { .. }));
    assert_eq!(
        first,
        deliver(&host, Uuid::new_v4(), &body)
            .await
            .expect("duplicate event")
    );
    let mut event: serde_json::Value = serde_json::from_slice(&body).expect("event");
    event["action"] = "synchronize".into();
    assert_eq!(
        first,
        deliver(
            &host,
            Uuid::new_v4(),
            &serde_json::to_vec(&event).expect("event")
        )
        .await
        .expect("same logical work")
    );
    drop(host);
    let restarted = self::host(directory.path());
    assert_eq!(identity, restarted.host_id().await.expect("same Host"));
    assert_eq!(
        first,
        deliver(&restarted, id, &body)
            .await
            .expect("lost acknowledgement replay")
    );
    let records = requests(&restarted, 0).await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].reported_head_sha, "b".repeat(40));
    assert_eq!(records[0].admitted_at_ms, 500);
    assert!(matches!(
        deliver(&restarted, id, &payload('c')).await,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Conflict))
    ));
}

#[tokio::test]
async fn bad_signatures_wrong_installations_and_cancelled_events_admit_nothing() {
    let (_directory, host, _) = fixture().await;
    let body = payload('b');
    let invalid = host
        .admit_github_review_webhook(
            GitHubReviewWebhook {
                delivery_id: Uuid::new_v4(),
                event: "pull_request",
                signature: &signature(&payload('c')),
                body: &body,
            },
            SECRET,
            0,
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        invalid,
        Err(LocalHostError::GitHubReview(
            GitHubReviewError::Authentication
        ))
    ));
    let mut event: serde_json::Value = serde_json::from_slice(&body).expect("event");
    event["installation"]["id"] = 99.into();
    assert!(matches!(
        deliver(
            &host,
            Uuid::new_v4(),
            &serde_json::to_vec(&event).expect("body")
        )
        .await,
        Err(LocalHostError::GitHubReview(
            GitHubReviewError::Authentication
        ))
    ));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let id = Uuid::new_v4();
    assert!(matches!(
        host.admit_github_review_webhook(
            GitHubReviewWebhook {
                delivery_id: id,
                event: "pull_request",
                signature: &signature(&body),
                body: &body,
            },
            SECRET,
            0,
            cancellation
        )
        .await,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Cancelled))
    ));
    assert!(requests(&host, 0).await.is_empty());
    assert!(matches!(
        deliver(&host, id, &body)
            .await
            .expect("cancelled admission can retry"),
        GitHubReviewAdmission::Queued { .. }
    ));
}

#[tokio::test]
async fn paused_policy_allows_manual_work_and_retries_retain_the_original_policy_snapshot() {
    let (_directory, host, mut policy) = fixture().await;
    let old = policy.clone();
    policy.enabled = false;
    set(&host, policy.clone(), Some(1)).await;
    let body = payload('b');
    assert_eq!(
        deliver(&host, Uuid::new_v4(), &body).await.expect("paused"),
        GitHubReviewAdmission::Ignored {
            reason: GitHubReviewSkip::Disabled
        }
    );
    let command = GitHubReviewCommand::Request {
        operation_id: Uuid::new_v4(),
        repository_id: 42,
        pull_number: 14,
        reported_base_sha: "a".repeat(40),
        reported_head_sha: "b".repeat(40),
    };
    let first = host
        .manage_github_review(command.clone(), 123, CancellationToken::new())
        .await
        .expect("manual");
    set(&host, old.clone(), Some(2)).await;
    assert_eq!(
        first,
        host.manage_github_review(command, 999, CancellationToken::new())
            .await
            .expect("exact replay")
    );
    let rows = requests(&host, 0).await;
    assert_eq!(rows[0].repository.revision, 2);
    assert_eq!(rows[0].repository.policy, policy);
    assert_eq!(rows[0].admitted_at_ms, 123);
    assert!(matches!(
        host.manage_github_review(
            GitHubReviewCommand::SetRepository {
                operation_id: Uuid::new_v4(),
                expected_revision: Some(1),
                policy: old,
            },
            1000,
            CancellationToken::new()
        )
        .await,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Conflict))
    ));
}

#[tokio::test]
async fn late_events_cannot_replace_a_newer_request_and_pages_have_stable_cursors() {
    let (_directory, host, _) = fixture().await;
    let latest = deliver(&host, Uuid::new_v4(), &payload('c'))
        .await
        .expect("newer first");
    deliver(&host, Uuid::new_v4(), &payload('b'))
        .await
        .expect("delayed older event");
    assert_eq!(
        latest,
        deliver(&host, Uuid::new_v4(), &payload('c'))
            .await
            .expect("same latest")
    );
    for pull_number in 15..35 {
        host.manage_github_review(
            GitHubReviewCommand::Request {
                operation_id: Uuid::new_v4(),
                repository_id: 42,
                pull_number,
                reported_base_sha: "a".repeat(40),
                reported_head_sha: "b".repeat(40),
            },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("manual request");
    }
    let first = requests(&host, 0).await;
    assert_eq!(first.len(), 20);
    assert_eq!(first[0].reported_head_sha, "c".repeat(40));
    assert_eq!(first[1].reported_head_sha, "b".repeat(40));
    let second = requests(&host, first.last().expect("last").sequence).await;
    assert_eq!(second.len(), 2);
    assert!(
        second
            .iter()
            .all(|row| !first.iter().any(|old| old.id == row.id))
    );
}

#[tokio::test]
async fn schema_nineteen_migrates_without_changing_existing_host_or_specialist() {
    let (directory, host, policy) = fixture().await;
    let identity = host.host_id().await.expect("identity");
    let db = catalog::open_verified(&host.config.database).expect("db");
    db.execute_batch("DROP TABLE host_review_deliveries; DROP TABLE host_review_requests; DROP TABLE host_review_operations; DROP TABLE host_review_repositories; UPDATE host_metadata SET schema_version=19; PRAGMA user_version=19;").expect("schema 19");
    drop(db);
    drop(host);
    let reopened = self::host(directory.path());
    assert_eq!(reopened.host_id().await.expect("identity"), identity);
    assert!(
        reopened
            .bot(policy.agent_id)
            .await
            .expect("specialist")
            .is_some()
    );
    set(&reopened, policy, None).await;
    deliver(&reopened, Uuid::new_v4(), &payload('b'))
        .await
        .expect("admission after migration");
}
