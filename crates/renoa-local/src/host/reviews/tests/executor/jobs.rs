use super::*;

#[tokio::test]
async fn a_failed_launch_survives_cleanup_and_restart_without_extending_its_deadline() {
    let (directory, first, id, api) = prepared("").await;
    let deadline = first.begin_github_review(id, 1_000).await.expect("admit");
    first
        .defer_github_review(id, 61_000, "systemd unavailable".to_owned())
        .await
        .expect("backoff");
    first
        .reap_github_review(id, 2_000)
        .await
        .expect("pre-start cleanup");
    drop(first);
    let reopened = host(directory.path());
    let jobs = reopened.github_review_work().await.expect("work");
    assert!(!jobs[0].finished);
    assert_eq!(jobs[0].started_at_ms, None);
    assert_eq!(jobs[0].retry_after_ms, 61_000);
    assert_eq!(jobs[0].last_error.as_deref(), Some("systemd unavailable"));
    assert_eq!(
        reopened
            .begin_github_review(id, 61_001)
            .await
            .expect("retry"),
        deadline
    );
    reopened
        .start_github_review(id, 61_001)
        .await
        .expect("worker entered");
    reopened
        .reap_github_review(id, 62_000)
        .await
        .expect("crashed worker cleanup");
    let jobs = reopened.github_review_work().await.expect("work");
    assert!(jobs[0].finished);
    assert_eq!(jobs[0].started_at_ms, Some(61_001));
    assert_eq!(jobs[0].deadline_at_ms, Some(deadline));
    api.stop().await;
}

#[tokio::test]
async fn never_started_work_expires_with_its_dispatch_failure_recorded() {
    let (_directory, host, id, api) = prepared("").await;
    let deadline = host.begin_github_review(id, 1_000).await.expect("admit");
    host.defer_github_review(id, 61_000, "launch file creation failed".to_owned())
        .await
        .expect("backoff");
    host.reap_github_review(id, deadline).await.expect("expire");
    let run = super::super::super::runs::get(
        &catalog::open_verified(&host.config.database).expect("db"),
        id,
    )
    .expect("get")
    .expect("run");
    assert!(
        matches!(run, GitHubReviewRun::Finished { outcome:GitHubReviewOutcome::Incomplete {reason}, .. } if reason.contains("60-minute") && reason.contains("launch file creation failed"))
    );
    api.stop().await;
}

#[tokio::test]
async fn schema_22_jobs_migrate_without_resetting_their_deadlines() {
    let (directory, first, id, api) = prepared("").await;
    first.begin_github_review(id, 1_000).await.expect("begin");
    let db = catalog::open_verified(&first.config.database).expect("db");
    db.execute_batch(
        "ALTER TABLE host_review_jobs DROP COLUMN started_at_ms;
        ALTER TABLE host_review_jobs DROP COLUMN retry_after_ms;
        ALTER TABLE host_review_jobs DROP COLUMN last_error;
        UPDATE host_metadata SET schema_version=22; PRAGMA user_version=22;",
    )
    .expect("old schema");
    drop(db);
    drop(first);
    let refused = try_host(directory.path());
    assert!(
        matches!(&refused, Err(LocalHostError::HostCatalog(crate::HostCatalogError::Invalid(message))) if message.contains("reset")),
        "an earlier data root must be refused until it is reset: {:?}",
        refused.as_ref().err()
    );
    crate::reset_host_data_root(&directory.path().join("data")).expect("cutover reset");
    let reopened = host(directory.path());
    assert!(
        reopened
            .github_review_work()
            .await
            .expect("discarded work")
            .is_empty(),
        "the cutover discards the admitted review job"
    );
    let policy = reviewer_policy(&reopened).await;
    set(&reopened, policy, None).await;
    let fresh = Uuid::new_v4();
    reopened
        .manage_github_review(
            GitHubReviewCommand::Request {
                operation_id: fresh,
                repository_id: 42,
                pull_number: 14,
                reported_base_sha: "a".repeat(40),
                reported_head_sha: "d".repeat(40),
            },
            100,
            CancellationToken::new(),
        )
        .await
        .expect("fresh admission");
    let deadline = reopened
        .begin_github_review(fresh, 1_000)
        .await
        .expect("fresh begin");
    assert_eq!(deadline, 1_000 + REVIEW_LIFETIME_MS);
    let work = reopened.github_review_work().await.expect("fresh work");
    assert_eq!(work[0].deadline_at_ms, Some(deadline));
    assert_eq!(work[0].started_at_ms, None);
    assert_eq!(work[0].retry_after_ms, 0);
    api.stop().await;
}
