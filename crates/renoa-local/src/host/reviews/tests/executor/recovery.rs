use super::*;
use crate::host::reviews::runs;

#[tokio::test]
async fn final_commit_failure_reuses_both_completed_model_stages() {
    let (directory, host, id, api) = prepared("").await;
    let db = catalog::open_verified(&host.config.database).expect("catalog");
    db.execute_batch("CREATE TRIGGER fail_review_result BEFORE UPDATE ON host_review_runs WHEN NEW.terminal=1 BEGIN SELECT RAISE(ABORT,'injected final result failure'); END;").expect("fault");
    assert!(execute(&host, id, &api).await.is_err());
    assert!(matches!(
        runs::get(&db, id).expect("run"),
        Some(GitHubReviewRun::Prepared { .. })
    ));
    let calls = fs::read(directory.path().join("auth.sqlite.calls")).expect("calls");
    fs::write(
        directory.path().join("model.mjs"),
        "throw Error('completed model stages must replay')",
    )
    .expect("disable model");
    db.execute_batch("DROP TRIGGER fail_review_result;")
        .expect("clear fault");
    let result = execute(&host, id, &api).await.expect("recover");
    assert!(matches!(
        result,
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Reviewed { .. },
            ..
        }
    ));
    assert_eq!(
        fs::read(directory.path().join("auth.sqlite.calls")).expect("calls"),
        calls
    );
    api.stop().await;
}

#[tokio::test]
async fn moving_pr_during_preparation_never_calls_model_and_retry_reconciles() {
    let (directory, host, id, api) = prepared("").await;
    api.state.lock().expect("state").change_at = Some(2);
    assert!(matches!(
        execute(&host, id, &api).await,
        Err(LocalHostError::GitHubReview(GitHubReviewError::MovingPull))
    ));
    assert!(!directory.path().join("auth.sqlite.calls").exists());
    let db = catalog::open_verified(&host.config.database).expect("db");
    assert_eq!(runs::get(&db, id).expect("run"), None);
    api.state.lock().expect("state").change_at = None;
    assert!(matches!(
        execute(&host, id, &api).await.expect("retry"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Reviewed { .. },
            ..
        }
    ));
    api.stop().await;
}

#[tokio::test]
async fn new_commit_after_investigation_preserves_findings_as_superseded() {
    let (_directory, host, id, api) = prepared("").await;
    api.state.lock().expect("state").change_at = Some(3);
    let result = execute(&host, id, &api).await.expect("review");
    let GitHubReviewRun::Finished {
        outcome: GitHubReviewOutcome::Superseded { report, .. },
        ..
    } = result
    else {
        panic!("expected superseded");
    };
    assert_eq!(report.findings.len(), 1);
    api.stop().await;
}

#[tokio::test]
async fn rate_limit_and_wrong_installation_remain_retryable_without_model_calls() {
    let (directory, host, id, api) = prepared("").await;
    api.state.lock().expect("state").status = Some(429);
    assert!(
        matches!(execute(&host,id,&api).await,Err(LocalHostError::GitHubReview(GitHubReviewError::Api {status:429,retry_after:Some(ref value)})) if value=="60")
    );
    {
        let mut state = api.state.lock().expect("state");
        state.status = None;
        state.installation = 99;
    }
    assert!(matches!(
        execute(&host, id, &api).await,
        Err(LocalHostError::GitHubReview(
            GitHubReviewError::Authentication
        ))
    ));
    assert!(!directory.path().join("auth.sqlite.calls").exists());
    api.stop().await;
}

#[tokio::test]
async fn closed_pr_skips_and_releases_inbox_capacity() {
    let (directory, host, id, api) = prepared("").await;
    api.state.lock().expect("state").closed = true;
    assert!(matches!(
        execute(&host, id, &api).await.expect("closed"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Skipped { .. },
            ..
        }
    ));
    assert!(!directory.path().join("auth.sqlite.calls").exists());
    let db = catalog::open_verified(&host.config.database).expect("db");
    let count:i64=db.query_row("SELECT count(*) FROM host_review_requests r WHERE NOT EXISTS(SELECT 1 FROM host_review_runs x WHERE x.request_id=r.id AND x.terminal=1)",[],|row|row.get(0)).expect("pending");
    assert_eq!(count, 0);
    api.stop().await;
}

#[tokio::test]
async fn policy_edit_before_execution_skips_without_network_or_inference() {
    let (directory, host, id, api) = prepared("").await;
    let db = catalog::open_verified(&host.config.database).expect("db");
    let repository = store::repository(&db, 42)
        .expect("repository")
        .expect("exists");
    let mut policy = repository.policy;
    policy.enabled = false;
    set(&host, policy, Some(repository.revision)).await;
    assert!(matches!(
        execute(&host, id, &api).await.expect("skip"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Skipped { .. },
            ..
        }
    ));
    assert!(!directory.path().join("auth.sqlite.calls").exists());
    assert!(api.state.lock().expect("state").requests.is_empty());
    api.stop().await;
}

#[tokio::test]
async fn oversized_preparation_is_durably_incomplete_without_a_model_call() {
    let (directory, host, id, api) = prepared("").await;
    api.state.lock().expect("state").changed_files = Some(501);
    let result = execute(&host, id, &api).await.expect("outcome");
    assert!(matches!(
        result,
        GitHubReviewRun::Finished {
            snapshot: None,
            outcome: GitHubReviewOutcome::Incomplete { .. },
            ..
        }
    ));
    let reply = host
        .manage_github_review(
            GitHubReviewCommand::Run { request_id: id },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("inspect");
    assert_eq!(
        reply,
        GitHubReviewReply::Run {
            record: Some(result)
        }
    );
    assert!(!directory.path().join("auth.sqlite.calls").exists());
    api.stop().await;
}

#[tokio::test]
async fn model_specification_drift_cannot_silently_change_a_frozen_run() {
    let (directory, host, id, api) = prepared("drift").await;
    assert!(matches!(
        execute(&host, id, &api).await,
        Err(LocalHostError::Configuration(_))
    ));
    assert!(!directory.path().join("auth.sqlite.calls").exists());
    fs::write(directory.path().join("auth.sqlite"), "").expect("restore model");
    assert!(matches!(
        execute(&host, id, &api).await.expect("recover"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Reviewed { .. },
            ..
        }
    ));
    api.stop().await;
}

#[tokio::test]
async fn schema_twenty_upgrade_preserves_pending_review_admission() {
    let (directory, first, id, api) = prepared("").await;
    let host_id = first.host_id().await.expect("host id");
    let db = catalog::open_verified(&first.config.database).expect("db");
    let request = store::get_request(&db, id).expect("request");
    db.execute_batch("DROP TABLE host_review_runs; UPDATE host_metadata SET schema_version=20; PRAGMA user_version=20;").expect("schema 20");
    drop(db);
    drop(first);
    let reopened = host(directory.path());
    assert_eq!(reopened.host_id().await.expect("host id"), host_id);
    let db = catalog::open_verified(&reopened.config.database).expect("db");
    assert_eq!(store::get_request(&db, id).expect("request"), request);
    assert!(matches!(
        execute(&reopened, id, &api)
            .await
            .expect("execute migrated request"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Reviewed { .. },
            ..
        }
    ));
    api.stop().await;
}

#[tokio::test]
async fn host_lease_and_cancellation_prevent_competing_execution() {
    let (directory, host, id, api) = prepared("").await;
    let lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(host.config.database.with_file_name(".reviews.lock"))
        .expect("lease");
    lease.try_lock().expect("lock");
    assert!(execute(&host, id, &api).await.is_err());
    lease.unlock().expect("release simulated owner");
    drop(lease);
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        host.execute_review_at(id, "private-app-jwt", cancellation, api.origin.clone())
            .await,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Cancelled))
    ));
    assert!(!directory.path().join("auth.sqlite.calls").exists());
    assert!(api.state.lock().expect("state").requests.is_empty());
    api.stop().await;
}

#[tokio::test]
async fn looping_model_stops_at_six_calls_with_incomplete_result() {
    let (directory, host, id, api) = prepared("exhaust").await;
    assert!(matches!(
        execute(&host, id, &api).await.expect("bounded outcome"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Incomplete { .. },
            ..
        }
    ));
    assert_eq!(
        fs::read_to_string(directory.path().join("auth.sqlite.calls"))
            .expect("calls")
            .lines()
            .count(),
        6
    );
    api.stop().await;
}
