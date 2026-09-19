use super::*;
use renoa_kernel::AgentId;
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, GitHubReviewCommand,
    GitHubReviewPolicy, LocalHostAdapters, LocalModelConfiguration, ModelProvider,
};
use std::collections::BTreeSet;

const ARCEE_PRESET_ID: &str = "renoa.personal.arcee.v1";
const SPECIALIST_PRESET_ID: &str = "renoa.specialist.v1";

async fn provision(
    host: &LocalHost,
    preset: &str,
    name: &str,
    instructions: Option<&str>,
) -> AgentId {
    let mut request = AgentCreateRequest::new(
        Uuid::new_v4(),
        AgentPresetId::new(preset).expect("preset id"),
        name,
    );
    if let Some(instructions) = instructions {
        request = request.with_instructions(instructions);
    }
    host.create_agent(
        AgentCreator::System {
            component: "github-service-test".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        request,
        CancellationToken::new(),
    )
    .await
    .expect("agent")
    .id
}

async fn host(root: &Path) -> LocalHost {
    let host = LocalHost::new(
        root,
        LocalModelConfiguration::new(
            root.join("unused.mjs"),
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "fixture",
            root.join("auth.sqlite"),
        ),
        LocalHostAdapters::new(None),
    )
    .expect("host");
    let operator = provision(&host, ARCEE_PRESET_ID, "Operator", None).await;
    let reviewer = host
        .create_agent(
            AgentCreator::Agent { agent_id: operator },
            AgentCreationOrigin::AgentTool,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
                "Reviewer",
            )
            .with_instructions("Review"),
            CancellationToken::new(),
        )
        .await
        .expect("reviewer")
        .id;
    host.manage_github_review(
        GitHubReviewCommand::SetRepository {
            operation_id: Uuid::new_v4(),
            expected_revision: None,
            policy: GitHubReviewPolicy {
                repository_id: 1,
                installation_id: 1,
                full_name: "owner/repo".to_owned(),
                agent_id: reviewer,
                enabled: true,
                triggers: BTreeSet::new(),
                skip_drafts: false,
            },
        },
        0,
        CancellationToken::new(),
    )
    .await
    .expect("repository");
    host
}

#[tokio::test]
async fn failed_cleanup_defers_one_job_but_preserves_global_ownership_checks() {
    let root = tempfile::tempdir().expect("Host");
    let host = host(root.path()).await;
    let ids = [Uuid::new_v4(), Uuid::new_v4()];
    for id in ids {
        host.manage_github_review(
            GitHubReviewCommand::Request {
                operation_id: id,
                repository_id: 1,
                pull_number: 1,
                reported_base_sha: "a".repeat(40),
                reported_head_sha: "b".repeat(40),
            },
            1,
            CancellationToken::new(),
        )
        .await
        .expect("request");
        host.begin_github_review(id, 100).await.expect("deadline");
        host.start_github_review(id, 101)
            .await
            .expect("worker entry");
    }
    let checkouts = root.path().join("review-workspaces");
    std::fs::create_dir_all(&checkouts).expect("checkouts");
    // A regular file where a directory is expected deterministically fails
    // remove_dir_all even when the test runs as root.
    std::fs::write(checkouts.join(ids[0].to_string()), "broken checkout").expect("bad cleanup");
    std::fs::create_dir(checkouts.join(ids[1].to_string())).expect("healthy checkout");
    let observations = observe_workers(
        host.github_review_work().await.expect("work"),
        &CancellationToken::new(),
        |id| async move {
            if id == ids[0] {
                Err(std::io::Error::other("unit query failed").into())
            } else {
                Ok(false)
            }
        },
    )
    .await;
    assert!(observations.uncertain);
    assert!(!observations.active);
    assert_eq!(observations.stopped.len(), 1);
    assert_eq!(observations.stopped[0].request_id, ids[1]);
    for job in observations.stopped {
        assert!(
            cleanup_stopped(&host, root.path(), job.request_id, 199)
                .await
                .expect("healthy cleanup")
        );
        assert!(matches!(
            host.publish_github_review(job.request_id, "unused", "bot", CancellationToken::new())
                .await
                .expect("suppression"),
            GitHubReviewPublication::Suppressed { .. }
        ));
    }
    assert!(checkouts.join(ids[0].to_string()).is_file());
    assert!(!checkouts.join(ids[1].to_string()).exists());
    let mut cleaned = Vec::new();
    for id in ids {
        if cleanup_stopped(&host, root.path(), id, 200)
            .await
            .expect("per-job outcome")
        {
            cleaned.push(id);
        }
    }
    assert_eq!(cleaned, vec![ids[1]]);
    let jobs = host.github_review_work().await.expect("work");
    assert!(!jobs[0].finished && jobs[0].last_error.is_some());
    assert_eq!(jobs[0].retry_after_ms, 60_200);
    // The healthy job was cleaned and its incomplete review suppressed, so it
    // no longer needs dispatch. The uncertain job remains durable and pending.
    assert_eq!(jobs.len(), 1);
    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.path().join(".reviews.lock"))
        .expect("lease");
    lease.try_lock().expect("active owner");
    assert!(
        cleanup_stopped(&host, root.path(), ids[0], 201)
            .await
            .is_err()
    );
    lease.unlock().expect("release");
    assert!(
        cleanup_stopped(&host, root.path(), Uuid::new_v4(), 201)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn stopped_launch_cleanup_allows_fresh_create_new_credentials() {
    let data = tempfile::tempdir().expect("data");
    let id = Uuid::new_v4();
    let root = data.path().join("github-executions").join(id.to_string());
    tokio::fs::create_dir_all(&root).await.expect("launch");
    private_write(&root.join("app.jwt"), b"expired credential")
        .await
        .expect("first dispatch");
    remove_launch(data.path(), id)
        .await
        .expect("confirmed stopped cleanup");
    tokio::fs::create_dir_all(&root)
        .await
        .expect("retry launch");
    private_write(&root.join("app.jwt"), b"fresh credential")
        .await
        .expect("retry dispatch");
    assert_eq!(
        tokio::fs::read(root.join("app.jwt")).await.expect("read"),
        b"fresh credential"
    );
}
