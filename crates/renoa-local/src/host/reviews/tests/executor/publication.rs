use super::*;
use crate::GitHubReviewPublication;

#[derive(Default)]
pub(super) enum Response {
    #[default]
    Normal,
    Lose,
    Reject,
}
pub(super) fn respond(headers: &str, body: &[u8], state: &mut ApiState) -> (u16, String) {
    if headers.starts_with("POST ") {
        if matches!(state.review_response, Response::Reject) {
            return (500, "{}".to_owned());
        }
        let body: serde_json::Value = serde_json::from_slice(body).expect("publication");
        assert_eq!(body["event"], "COMMENT");
        assert_eq!(
            body["commit_id"],
            state
                .commits
                .as_ref()
                .map_or_else(|| "b".repeat(40), |c| c.1.clone())
        );
        let review = serde_json::json!({
            "id": 123, "body":body["body"], "commit_id":body["commit_id"],
            "html_url":"https://github.com/owner/repository/pull/14#pullrequestreview-123",
            "user":{"login":"soundwave[bot]"}
        });
        state.reviews.push(review.clone());
        if matches!(state.review_response, Response::Lose) {
            return (500, "{}".to_owned());
        }
        return (200, review.to_string());
    }
    (200, serde_json::to_string(&state.reviews).expect("reviews"))
}

pub(super) async fn publish(
    host: &LocalHost,
    id: Uuid,
    api: &Api,
) -> Result<GitHubReviewPublication, LocalHostError> {
    host.publish_review_at(
        id,
        "private-app-jwt",
        "soundwave[bot]",
        api.origin.clone(),
        CancellationToken::new(),
    )
    .await
}

#[tokio::test]
async fn publishes_exact_commit_and_replays_without_another_github_call() {
    let (_directory, host, id, api) = prepared("").await;
    execute(&host, id, &api).await.expect("review");
    let first = publish(&host, id, &api).await.expect("publish");
    assert!(matches!(
        first,
        GitHubReviewPublication::Published { review_id: 123, .. }
    ));
    let count = api.state.lock().expect("state").requests.len();
    assert_eq!(first, publish(&host, id, &api).await.expect("replay"));
    assert!(matches!(
        host.manage_github_review(
            GitHubReviewCommand::Publication { request_id: id },
            0,
            CancellationToken::new()
        )
        .await
        .expect("management read"),
        GitHubReviewReply::Publication {
            record: Some(GitHubReviewPublication::Published { review_id: 123, .. })
        }
    ));
    assert_eq!(api.state.lock().expect("state").requests.len(), count);
    {
        let state = api.state.lock().expect("state");
        let body: serde_json::Value = serde_json::from_str(
            state
                .bodies
                .iter()
                .find(|b| b.contains("\"event\":\"COMMENT\""))
                .expect("POST"),
        )
        .expect("payload");
        let summary = body["body"].as_str().expect("summary");
        assert!(summary.starts_with("Soundwave reporting."));
        assert!(summary.contains("<summary>Review limitations</summary>"));
        assert!(summary.contains("no findings is not proof of correctness"));
        assert_eq!(body["comments"][0]["line"], 2);
        assert_eq!(body["comments"][0]["side"], "RIGHT");
        assert!(
            body["comments"][0]["body"]
                .as_str()
                .expect("body")
                .contains("[P1]")
        );
    }
    api.stop().await;
}

#[tokio::test]
async fn lost_publication_acknowledgement_reconciles_once_after_restart() {
    let (directory, first, id, api) = prepared("").await;
    execute(&first, id, &api).await.expect("review");
    api.state.lock().expect("state").review_response = Response::Lose;
    assert!(publish(&first, id, &api).await.is_err());
    drop(first);
    let recovered = publish(&host(directory.path()), id, &api)
        .await
        .expect("reconcile");
    assert!(matches!(
        recovered,
        GitHubReviewPublication::Published { review_id: 123, .. }
    ));
    assert_eq!(api.state.lock().expect("state").reviews.len(), 1);
    api.stop().await;
}

#[tokio::test]
async fn unknown_publication_does_not_blindly_retry_and_changed_head_is_suppressed() {
    let (_directory, host, id, api) = prepared("").await;
    execute(&host, id, &api).await.expect("review");
    api.state.lock().expect("state").review_response = Response::Reject;
    assert!(publish(&host, id, &api).await.is_err());
    assert!(matches!(
        publish(&host, id, &api).await.expect("unknown"),
        GitHubReviewPublication::NeedsAttention { .. }
    ));
    let posts = api
        .state
        .lock()
        .expect("state")
        .bodies
        .iter()
        .filter(|b| b.contains("\"event\":\"COMMENT\""))
        .count();
    assert_eq!(posts, 1);
    api.stop().await;

    let (_directory, host, id, api) = prepared("").await;
    execute(&host, id, &api).await.expect("review");
    api.state.lock().expect("state").change_at = Some(0);
    assert!(matches!(
        publish(&host, id, &api).await.expect("changed"),
        GitHubReviewPublication::Suppressed { .. }
    ));
    assert!(api.state.lock().expect("state").reviews.is_empty());
    api.stop().await;
}

#[tokio::test]
async fn deadline_and_cleanup_survive_restart_without_deleting_an_active_checkout() {
    let (directory, first, id, api) = prepared("").await;
    assert_eq!(
        first.github_review_work().await.expect("work")[0].deadline_at_ms,
        None
    );
    let deadline = first.begin_github_review(id, 1000).await.expect("begin");
    assert_eq!(deadline, 3_601_000);
    drop(first);
    let host = host(directory.path());
    assert_eq!(
        host.begin_github_review(id, 99_000)
            .await
            .expect("resume deadline"),
        deadline
    );
    let root = host
        .config
        .database
        .with_file_name("review-workspaces")
        .join(id.to_string());
    fs::create_dir_all(&root).expect("checkout");
    fs::write(root.join("source"), "data").expect("source");
    let launch = host
        .config
        .database
        .with_file_name("github-executions")
        .join(id.to_string());
    fs::create_dir_all(&launch).expect("launcher");
    fs::write(launch.join("app.jwt"), "private-jwt").expect("credential");
    let lease = crate::host::lease::ExecutionLease::acquire(
        &host.config.database.with_file_name(".reviews.lock"),
    )
    .expect("live worker");
    assert!(host.reap_github_review(id, deadline).await.is_err());
    assert!(root.exists());
    assert!(launch.exists());
    drop(lease);
    host.reap_github_review(id, deadline)
        .await
        .expect("deadline cleanup");
    assert!(!root.exists());
    assert!(!launch.exists());
    host.reap_github_review(id, deadline + 1000)
        .await
        .expect("idempotent cleanup");
    let run = super::super::super::runs::get(
        &catalog::open_verified(&host.config.database).expect("db"),
        id,
    )
    .expect("run")
    .expect("terminal");
    assert!(
        matches!(run, GitHubReviewRun::Finished { outcome: GitHubReviewOutcome::Incomplete { ref reason }, .. }
        if reason.contains("60-minute"))
    );
    assert!(host.github_review_work().await.expect("publication work")[0].finished);
    host.defer_github_publication(id, deadline + 60_000)
        .await
        .expect("persist backoff");
    drop(host);
    assert_eq!(
        super::super::host(directory.path())
            .github_review_work()
            .await
            .expect("restart backoff")[0]
            .publish_after_ms,
        deadline + 60_000
    );
    api.stop().await;
}

#[tokio::test]
async fn queued_old_commits_are_skipped_using_live_github_state_even_when_delivered_late() {
    let (directory, host, _manual, api) = prepared("").await;
    // The current commit arrives first; delayed older events must not trigger
    // another expensive review of that same current head.
    let GitHubReviewAdmission::Queued {
        request_id: current,
    } = deliver(&host, Uuid::new_v4(), &payload('b'))
        .await
        .expect("current event")
    else {
        panic!("queued");
    };
    let GitHubReviewAdmission::Queued { request_id: old } =
        deliver(&host, Uuid::new_v4(), &payload('d'))
            .await
            .expect("late event")
    else {
        panic!("queued");
    };
    assert!(matches!(
        execute(&host, old, &api).await.expect("coalesce"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Skipped { .. },
            ..
        }
    ));
    assert!(!directory.path().join("auth.sqlite.calls").exists());
    assert!(matches!(
        execute(&host, current, &api).await.expect("current review"),
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Reviewed { .. },
            ..
        }
    ));
    api.stop().await;
}
