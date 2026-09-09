use super::*;
use crate::{HostObserver, HostReviewControl, ReviewPolicyUpdate};

#[tokio::test]
async fn owner_policy_changes_share_admission_rules_and_replay_after_later_edits() {
    let (directory, host, policy) = fixture().await;
    let root = directory.path().join("data");
    let host_id = host.host_id().await.expect("identity");
    let owner = Uuid::new_v4();
    let control = HostReviewControl::open(&root, host_id, owner).expect("control");
    let original = deliver(&host, Uuid::new_v4(), &payload('b'))
        .await
        .expect("admit");
    let request = ReviewPolicyUpdate {
        operation_id: Uuid::new_v4(),
        expected_revision: 1,
        enabled: false,
        triggers: policy.triggers.clone(),
        skip_drafts: false,
    };
    assert!(matches!(
        control
            .update_policy(Uuid::new_v4(), 42, request.clone())
            .await,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Forbidden))
    ));
    let saved = control
        .update_policy(owner, 42, request.clone())
        .await
        .expect("owner change");
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.policy.agent_id, policy.agent_id);
    assert!(matches!(
        deliver(&host, Uuid::new_v4(), &payload('c'))
            .await
            .expect("admission"),
        GitHubReviewAdmission::Ignored {
            reason: GitHubReviewSkip::Disabled
        }
    ));
    set(&host, policy.clone(), Some(2)).await;
    let restarted = HostReviewControl::open(&root, host_id, owner).expect("restart");
    assert_eq!(
        restarted
            .update_policy(owner, 42, request.clone())
            .await
            .expect("lost ack replay"),
        saved
    );
    let mut changed = request.clone();
    changed.enabled = true;
    assert!(matches!(
        restarted.update_policy(owner, 42, changed).await,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Conflict))
    ));
    let mut stale = request.clone();
    stale.operation_id = Uuid::new_v4();
    assert!(matches!(
        restarted.update_policy(owner, 42, stale).await,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Conflict))
    ));
    let observer = HostObserver::open(&root).expect("observer");
    let snapshot = observer.snapshot().await.expect("current state");
    assert_eq!(snapshot.review_repositories[0].revision, 3);
    assert!(snapshot.review_repositories[0].policy.enabled);
    let GitHubReviewAdmission::Queued { request_id } = original else {
        panic!("queued");
    };
    assert_eq!(
        observer
            .review_detail(request_id)
            .await
            .expect("detail")
            .expect("known")
            .repository
            .revision,
        1
    );
    // The data path alone cannot authorize a replacement Host, even on replay.
    let db = catalog::open_verified(&root.join(catalog::HOST_DATABASE)).expect("db");
    db.execute(
        "UPDATE host_identity SET host_id=?1",
        [Uuid::new_v4().to_string()],
    )
    .expect("replacement");
    assert!(restarted.update_policy(owner, 42, request).await.is_err());
}

#[tokio::test]
async fn observation_separates_retry_publication_and_captured_policy_without_payloads() {
    let (directory, host, _) = fixture().await;
    let GitHubReviewAdmission::Queued { request_id } =
        deliver(&host, Uuid::new_v4(), &payload('b'))
            .await
            .expect("admit")
    else {
        panic!("queued");
    };
    host.begin_github_review(request_id, 100)
        .await
        .expect("job");
    host.start_github_review(request_id, 101)
        .await
        .expect("start");
    host.defer_github_review(request_id, 500, "provider unavailable".into())
        .await
        .expect("retry");
    let root = directory.path().join("data");
    let db = catalog::open_verified(&root.join(catalog::HOST_DATABASE)).expect("db");
    db.execute(
        "INSERT INTO host_review_publications VALUES(?1,0,?2)",
        rusqlite::params![
            request_id.to_string(),
            serde_json::json!({"state":"sending","payload":{"body":"PRIVATE POST PAYLOAD"}})
                .to_string()
        ],
    )
    .expect("uncertain publication boundary");
    let observer = HostObserver::open(&root).expect("observer");
    let summary = serde_json::to_value(observer.snapshot().await.expect("summary")).expect("json");
    assert_eq!(summary["reviews"][0]["publication"], "sending");
    assert_eq!(summary["reviews"][0]["worker_error"], true);
    assert_eq!(summary["reviews"][0]["retry_after_ms"], 500);
    assert!(!summary.to_string().contains("provider unavailable"));
    let detail = serde_json::to_value(observer.review_detail(request_id).await.expect("detail"))
        .expect("json");
    assert_eq!(detail["publication"]["state"], "sending");
    assert_eq!(detail["execution"]["last_error"], "provider unavailable");
    assert_eq!(detail["repository"]["revision"], 1);
    assert!(!detail.to_string().contains("PRIVATE POST PAYLOAD"));
    db.execute(
        "UPDATE host_review_publications SET record_json=?1",
        [serde_json::json!({"state":"unexpected_future_state"}).to_string()],
    )
    .expect("incompatible boundary");
    assert!(observer.snapshot().await.is_err());
    assert!(observer.review_detail(request_id).await.is_err());
}
