use super::*;
use crate::host::reviews::runs;

#[tokio::test]
async fn a_review_survives_repeated_compaction_and_keeps_prioritized_evidence() {
    let (directory, host, id, api) = prepared("compactions").await;
    api.state.lock().expect("state").large_source = true;
    let result = execute(&host, id, &api)
        .await
        .expect("review through compaction");
    let GitHubReviewRun::Finished {
        outcome: GitHubReviewOutcome::Reviewed { report, usage },
        ..
    } = result
    else {
        panic!("review must finish: {result:?}");
    };
    assert_eq!(report.findings[0].priority, Some(crate::ReviewPriority::P1));
    assert_eq!(report.findings[0].evidence.quote, "    10 / count");
    let summaries = fs::read_to_string(directory.path().join("auth.sqlite.compactions"))
        .expect("summary calls")
        .lines()
        .count();
    let normal = fs::read_to_string(directory.path().join("auth.sqlite.calls"))
        .expect("review calls")
        .lines()
        .count();
    assert_eq!(
        usage.expect("complete usage including compaction").input,
        u64::try_from((summaries + normal) * 10).expect("small fixture usage")
    );
    assert!(
        fs::read_to_string(directory.path().join("auth.sqlite.compactions"))
            .expect("compactions")
            .lines()
            .count()
            >= 2
    );
    let kernel = renoa_kernel::Kernel::open(
        host.config
            .database
            .with_file_name("review-sessions")
            .join(id.to_string())
            .join("kernel.sqlite"),
    )
    .expect("kernel");
    let events = kernel
        .events_after(
            renoa_kernel::SessionId::from_uuid(id),
            renoa_kernel::EventCursor::START,
        )
        .expect("durable history")
        .events;
    assert!(
        events
            .iter()
            .filter(|event| event.kind == renoa_agent_loop::CONTEXT_CHECKPOINT_EVENT_KIND)
            .count()
            >= 2
    );
    api.stop().await;
}

#[tokio::test]
async fn accumulated_source_context_can_cross_the_old_cap_and_still_validate() {
    let (directory, host, id, api) = prepared("large-batch").await;
    api.state.lock().expect("state").large_source = true;
    assert!(matches!(
        execute(&host, id, &api).await.expect("large review"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Reviewed { .. },
            ..
        }
    ));
    let request: renoa_agent::ModelRequest = serde_json::from_slice(
        &fs::read(directory.path().join("auth.sqlite.last-request")).expect("final model input"),
    )
    .expect("model request");
    let estimated = crate::model_context::estimate_input_tokens(&request);
    assert!(
        (100_001..258_400).contains(&estimated),
        "estimated {estimated}"
    );
    api.stop().await;
}

#[tokio::test]
async fn source_batches_complete_both_stages_and_oversized_batches_retain_the_failure() {
    for mode in ["batch", "oversized-batch"] {
        let (_directory, host, id, api) = prepared(mode).await;
        let result = execute(&host, id, &api).await.expect("review outcome");
        let GitHubReviewRun::Finished { outcome, .. } = result else {
            panic!("expected finished review");
        };
        if mode == "batch" {
            assert!(matches!(outcome, GitHubReviewOutcome::Reviewed { .. }));
        } else {
            assert!(matches!(outcome, GitHubReviewOutcome::Incomplete { reason }
                if reason == "Investigation failed: model returned 51 tool calls; the per-turn limit is 50"));
        }
        api.stop().await;
    }
}

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
    api.state.lock().expect("state").change_at = Some(4);
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
async fn remote_file_count_does_not_reject_a_review_before_inspection() {
    let (directory, host, id, api) = prepared("").await;
    api.state.lock().expect("state").changed_files = Some(501);
    let result = execute(&host, id, &api).await.expect("outcome");
    assert!(matches!(
        result,
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Reviewed { .. },
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
    assert!(directory.path().join("auth.sqlite.calls").exists());
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
async fn investigation_and_validation_can_both_continue_past_six_responses() {
    let (directory, host, id, api) = prepared("exhaust").await;
    assert!(matches!(
        execute(&host, id, &api).await.expect("completed outcome"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Reviewed { .. },
            ..
        }
    ));
    assert_eq!(
        fs::read_to_string(directory.path().join("auth.sqlite.calls"))
            .expect("calls")
            .lines()
            .count(),
        18
    );
    api.stop().await;
}
