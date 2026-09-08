use super::*;
use crate::GitHubReviewPublication;

#[tokio::test]
async fn incomplete_reviews_stay_in_host_state_without_any_github_request() {
    let (directory, host, id, api) = prepared("invalid").await;
    let run = execute(&host, id, &api).await.expect("incomplete run");
    assert!(matches!(
        &run,
        GitHubReviewRun::Finished {
            outcome: GitHubReviewOutcome::Incomplete { .. },
            ..
        }
    ));
    let requests = api.state.lock().expect("state").requests.len();
    api.state.lock().expect("state").status = Some(401);
    let publication = host
        .publish_review_at(
            id,
            "unusable credential",
            "soundwave[bot]",
            api.origin.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("local suppression");
    assert!(matches!(
        &publication,
        GitHubReviewPublication::Suppressed { .. }
    ));
    drop(host);
    let reopened = super::super::host(directory.path());
    assert_eq!(
        reopened
            .publish_review_at(
                id,
                "still unusable",
                "soundwave[bot]",
                api.origin.clone(),
                CancellationToken::new()
            )
            .await
            .expect("suppression replay"),
        publication
    );
    assert_eq!(api.state.lock().expect("state").requests.len(), requests);
    assert!(api.state.lock().expect("state").reviews.is_empty());
    let saved = reopened
        .manage_github_review(
            GitHubReviewCommand::Run { request_id: id },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("Host management result");
    assert!(matches!(saved,GitHubReviewReply::Run {record:Some(ref retained)} if retained == &run));
    api.stop().await;
}
