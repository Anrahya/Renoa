use super::*;
use renoa_local::{GitHubReviewCommand, GitHubReviewPolicy, GitHubReviewTrigger};

#[tokio::test]
async fn policy_http_requires_owner_origin_and_shares_durable_revision_rules() {
    let f = Fixture::new().await;
    f.host
        .manage_github_review(
            GitHubReviewCommand::SetRepository {
                operation_id: Uuid::new_v4(),
                expected_revision: None,
                policy: GitHubReviewPolicy {
                    repository_id: 42,
                    installation_id: 7,
                    full_name: "owner/repo".into(),
                    agent_id: f.routine.spec.agent_id,
                    enabled: true,
                    triggers: [GitHubReviewTrigger::Synchronize].into(),
                    skip_drafts: false,
                },
            },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("configure");
    let url = format!("{}/v1/host/repositories/42/policy", f.url);
    let body = json!({"operation_id":Uuid::new_v4(),"expected_revision":1,"enabled":true,"triggers":["opened"],"skip_drafts":true});
    let status = f
        .client
        .post(&url)
        .header("origin", ORIGIN)
        .json(&body)
        .send()
        .await
        .expect("unauthenticated")
        .status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let status = f
        .client
        .post(&url)
        .header("cookie", &f.cookie)
        .header("origin", "https://other.invalid")
        .json(&body)
        .send()
        .await
        .expect("cross origin")
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);
    let send = |body: Value| {
        f.client
            .post(&url)
            .header("cookie", &f.cookie)
            .header("origin", ORIGIN)
            .json(&body)
            .send()
    };
    let response = send(body.clone()).await.expect("owner write");
    assert_eq!(response.status(), StatusCode::OK);
    let saved: Value = response.json().await.expect("receipt");
    assert_eq!(saved["record"]["revision"], 2);
    assert_eq!(saved["record"]["policy"]["skip_drafts"], true);
    assert_eq!(
        send(body.clone())
            .await
            .expect("replay")
            .json::<Value>()
            .await
            .expect("receipt"),
        saved
    );
    let mut stale = body.clone();
    stale["operation_id"] = json!(Uuid::new_v4());
    assert_eq!(
        send(stale).await.expect("stale").status(),
        StatusCode::CONFLICT
    );
    let mut invalid = body;
    invalid["agent_id"] = json!(Uuid::new_v4());
    assert_eq!(
        send(invalid).await.expect("untrusted actor").status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let observed: Value = f
        .client
        .get(format!("{}/v1/host", f.url))
        .header("cookie", &f.cookie)
        .send()
        .await
        .expect("read")
        .json()
        .await
        .expect("snapshot");
    assert_eq!(observed["review_repositories"][0], saved["record"]);
    f.finish().await;
}
