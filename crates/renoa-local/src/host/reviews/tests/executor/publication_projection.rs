use super::*;
use crate::GitHubReviewPublication;

#[tokio::test]
async fn oversized_github_messages_preserve_the_host_report_and_reconcile_once() {
    let (directory, first, id, api) = prepared("large-report").await;
    let run = execute(&first, id, &api).await.expect("complete review");
    let GitHubReviewRun::Finished {
        outcome: GitHubReviewOutcome::Reviewed { report, .. },
        ..
    } = &run
    else {
        panic!("reviewed: {run:?}")
    };
    assert_eq!(report.findings.len(), 1);
    assert!(report.findings[0].trigger.chars().count() > 65_536);
    assert!(
        report
            .limitations
            .iter()
            .any(|s| s.chars().count() > 65_536)
    );
    api.state.lock().expect("state").review_response = publication::Response::Lose;
    assert!(publication::publish(&first, id, &api).await.is_err());
    assert_eq!(api.state.lock().expect("state").reviews.len(), 1);
    {
        let mut state = api.state.lock().expect("state");
        let mut unrelated = state.reviews[0].clone();
        unrelated["body"] = "🦀".repeat(65_536).into();
        unrelated["id"] = 122.into();
        state.reviews.insert(0, unrelated);
    }
    drop(first);
    let reopened = host(directory.path());
    let published = publication::publish(&reopened, id, &api)
        .await
        .expect("reconcile accepted preview");
    assert!(matches!(
        published,
        GitHubReviewPublication::Published { .. }
    ));
    assert_eq!(
        published,
        publication::publish(&reopened, id, &api)
            .await
            .expect("settled replay")
    );
    let retained = reopened
        .manage_github_review(
            GitHubReviewCommand::Run { request_id: id },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("full Host report");
    assert!(matches!(retained, GitHubReviewReply::Run { record: Some(saved) } if saved == run));
    {
        let state = api.state.lock().expect("state");
        let posts: Vec<serde_json::Value> = state
            .bodies
            .iter()
            .filter(|b| b.contains("\"event\":\"COMMENT\""))
            .map(|b| serde_json::from_str(b).expect("JSON"))
            .collect();
        assert_eq!(posts.len(), 1);
        assert!(
            state
                .requests
                .iter()
                .any(|path| path.contains("per_page=1&page=2"))
        );
        let body = posts[0]["body"].as_str().expect("body");
        let comment = posts[0]["comments"][0]["body"].as_str().expect("comment");
        for text in [body, comment] {
            assert!(text.chars().count() <= 65_536);
            assert!(text.contains("GitHub preview:"));
            assert!(text.contains(&id.to_string()));
            assert!(text.contains("&lt;"));
            assert!(text.contains('🦀'));
            assert!(
                text.contains("[Preview ends; see the Host record for the complete text.]</pre>")
            );
        }
        assert!(body.starts_with("Soundwave reporting."));
        assert!(body.ends_with(&format!("<!-- renoa-review:{id} -->")));
        assert!(comment.contains("finding 1"));
        assert_eq!(posts[0]["comments"][0]["line"], 2);
    }
    api.stop().await;
}
