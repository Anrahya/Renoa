use super::*;

async fn worker(host: &LocalHost, id: Uuid, api: &Api) -> Result<GitHubReviewRun, LocalHostError> {
    host.execute_review_worker(
        id,
        "private-app-jwt",
        None,
        CancellationToken::new(),
        api.origin.clone(),
    )
    .await
}

#[tokio::test]
async fn worker_api_failure_survives_reaping_and_restart_then_retries() {
    let (directory, first, id, api) = prepared("").await;
    api.state.lock().expect("state").status = Some(429);
    assert!(worker(&first, id, &api).await.is_err());
    let job = first.github_review_work().await.expect("work").remove(0);
    let deadline = job.deadline_at_ms.expect("deadline");
    first
        .reap_github_review(id, deadline - 1)
        .await
        .expect("post-stop cleanup");
    drop(first);
    let reopened = host(directory.path());
    let job = reopened.github_review_work().await.expect("work").remove(0);
    assert!(
        !job.finished,
        "a started worker's temporary API failure must stay retryable"
    );
    assert_eq!(job.started_at_ms, None);
    assert!(job.retry_after_ms > 0);
    assert!(job.last_error.as_deref().expect("cause").contains("429"));
    api.state.lock().expect("state").status = None;
    assert!(matches!(
        worker(&reopened, id, &api).await.expect("retry"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Reviewed { .. },
            ..
        }
    ));
    assert_eq!(
        reopened.github_review_work().await.expect("work")[0].deadline_at_ms,
        Some(deadline)
    );
    api.stop().await;
}

#[tokio::test]
async fn permanent_worker_errors_retain_the_cause_through_cleanup_and_restart() {
    let (directory, first, id, api) = prepared("").await;
    api.state.lock().expect("state").status = Some(401);
    assert!(worker(&first, id, &api).await.is_err());
    let job = first.github_review_work().await.expect("work").remove(0);
    assert!(job.finished);
    assert_eq!(job.retry_after_ms, 0);
    first
        .reap_github_review(id, job.deadline_at_ms.expect("deadline") - 1)
        .await
        .expect("reap");
    drop(first);
    let reopened = host(directory.path());
    let GitHubReviewReply::Run {
        record:
            Some(GitHubReviewRun::Finished {
                outcome: GitHubReviewOutcome::Incomplete { reason },
                ..
            }),
    } = reopened
        .manage_github_review(
            GitHubReviewCommand::Run { request_id: id },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("run")
    else {
        panic!("specific incomplete result")
    };
    assert!(reason.contains("401"));
    assert!(!reason.contains("private-app-jwt"));
    let requests = api.state.lock().expect("state").requests.len();
    assert!(matches!(
        reopened
            .publish_review_at(
                id,
                "unusable",
                "soundwave[bot]",
                api.origin.clone(),
                CancellationToken::new()
            )
            .await
            .expect("suppressed"),
        crate::GitHubReviewPublication::Suppressed { .. }
    ));
    assert_eq!(api.state.lock().expect("state").requests.len(), requests);
    api.stop().await;
}

#[tokio::test]
async fn a_worker_retry_expires_with_its_cause_and_explicit_model_outcomes_stay_terminal() {
    let (_directory, host, id, api) = prepared("").await;
    let now = crate::TurnObservation::now()
        .expect("clock")
        .unix_milliseconds();
    let deadline = host
        .begin_github_review(id, now - crate::REVIEW_LIFETIME_MS + 30_000)
        .await
        .expect("existing lifetime");
    api.state.lock().expect("state").status = Some(503);
    assert!(worker(&host, id, &api).await.is_err());
    let job = host.github_review_work().await.expect("work").remove(0);
    assert_eq!(job.deadline_at_ms, Some(deadline));
    assert_eq!(
        job.retry_after_ms, deadline,
        "a longer backoff wakes only to expire, not to retry early"
    );
    host.reap_github_review(id, deadline).await.expect("expiry");
    let result = host
        .manage_github_review(
            GitHubReviewCommand::Run { request_id: id },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("run");
    assert!(
        matches!(result,GitHubReviewReply::Run {record:Some(GitHubReviewRun::Finished{outcome:GitHubReviewOutcome::Incomplete{reason},..})} if reason.contains("60-minute") && reason.contains("503"))
    );
    api.stop().await;

    let (_directory, host, id, api) = prepared("invalid").await;
    let run = worker(&host, id, &api)
        .await
        .expect("explicit model outcome");
    assert!(matches!(
        run,
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Incomplete { .. },
            ..
        }
    ));
    let job = host.github_review_work().await.expect("work").remove(0);
    assert!(job.finished && job.started_at_ms.is_some());
    assert_eq!(job.retry_after_ms, 0);
    host.reap_github_review(id, job.deadline_at_ms.expect("deadline") - 1)
        .await
        .expect("reap");
    api.stop().await;
}
