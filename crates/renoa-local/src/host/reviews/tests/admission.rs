use super::*;

#[tokio::test]
async fn policy_filters_are_durable_and_enabling_later_does_not_reinterpret_a_receipt() {
    let (_directory, host, mut policy) = fixture().await;
    for (field, value, reason) in [
        ("draft", serde_json::json!(true), GitHubReviewSkip::Draft),
        (
            "state",
            serde_json::json!("closed"),
            GitHubReviewSkip::Closed,
        ),
    ] {
        let mut event: serde_json::Value = serde_json::from_slice(&payload('b')).expect("event");
        event["pull_request"][field] = value;
        let body = serde_json::to_vec(&event).expect("body");
        let id = Uuid::new_v4();
        assert_eq!(
            deliver(&host, id, &body).await.expect("filtered"),
            GitHubReviewAdmission::Ignored { reason }
        );
    }
    policy.triggers.clear();
    set(&host, policy.clone(), Some(1)).await;
    let id = Uuid::new_v4();
    let ignored = deliver(&host, id, &payload('b'))
        .await
        .expect("trigger disabled");
    assert_eq!(
        ignored,
        GitHubReviewAdmission::Ignored {
            reason: GitHubReviewSkip::TriggerDisabled
        }
    );
    policy.triggers.insert(GitHubReviewTrigger::Opened);
    set(&host, policy, Some(2)).await;
    assert_eq!(
        deliver(&host, id, &payload('b'))
            .await
            .expect("replay original receipt"),
        ignored
    );
    assert!(requests(&host, 0).await.is_empty());
    assert!(matches!(
        deliver(&host, Uuid::new_v4(), &payload('b'))
            .await
            .expect("new enabled event"),
        GitHubReviewAdmission::Queued { .. }
    ));
}

#[tokio::test]
async fn failed_receipt_commit_rolls_back_work_and_concurrent_retries_converge() {
    let (_directory, host, _) = fixture().await;
    let db = catalog::open_verified(&host.config.database).expect("db");
    db.execute_batch("CREATE TRIGGER failed_receipt BEFORE INSERT ON host_review_deliveries BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").expect("failure boundary");
    let id = Uuid::new_v4();
    let body = payload('b');
    assert!(deliver(&host, id, &body).await.is_err());
    assert!(requests(&host, 0).await.is_empty());
    db.execute_batch("DROP TRIGGER failed_receipt;")
        .expect("restore boundary");
    let (first, second) = tokio::join!(deliver(&host, id, &body), deliver(&host, id, &body));
    assert_eq!(
        first.expect("first admission"),
        second.expect("concurrent replay")
    );
    assert_eq!(requests(&host, 0).await.len(), 1);
}

#[tokio::test]
async fn changed_event_headers_malformed_targets_and_large_payloads_are_rejected() {
    let (_directory, host, _) = fixture().await;
    let body = payload('b');
    let id = Uuid::new_v4();
    deliver(&host, id, &body).await.expect("first");
    let changed = host
        .admit_github_review_webhook(
            GitHubReviewWebhook {
                delivery_id: id,
                event: "ping",
                signature: &signature(&body),
                body: &body,
            },
            SECRET,
            0,
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        changed,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Conflict))
    ));
    for content in [payload('z'), vec![b' '; 1024 * 1024 + 1]] {
        assert!(deliver(&host, Uuid::new_v4(), &content).await.is_err());
    }
    let mut foreign: serde_json::Value = serde_json::from_slice(&body).expect("event");
    foreign["repository"]["id"] = 999.into();
    assert_eq!(
        deliver(
            &host,
            Uuid::new_v4(),
            &serde_json::to_vec(&foreign).expect("body")
        )
        .await
        .expect("foreign repo"),
        GitHubReviewAdmission::Ignored {
            reason: GitHubReviewSkip::UnconfiguredRepository
        }
    );
    assert_eq!(requests(&host, 0).await.len(), 1);
}

#[tokio::test]
async fn full_inbox_rejects_new_work_without_breaking_prior_acknowledgement_replay() {
    let (_directory, host, _) = fixture().await;
    let original = deliver(&host, Uuid::new_v4(), &payload('b'))
        .await
        .expect("first");
    let db = catalog::open_verified(&host.config.database).expect("db");
    // Populate the remaining pending inbox through the same persisted shape,
    // avoiding 1,023 filesystem reopenings merely to reach the capacity boundary.
    db.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<1023)
        INSERT INTO host_review_requests(id,repository_id,repository_json,pull_number,base_sha,head_sha,admitted_at_ms)
        SELECT printf('00000000-0000-0000-0000-%012d',i),repository_id,repository_json,100+i,base_sha,head_sha,0 FROM n CROSS JOIN host_review_requests WHERE sequence=1;").expect("fill inbox");
    assert!(matches!(
        deliver(&host, Uuid::new_v4(), &payload('c')).await,
        Err(LocalHostError::GitHubReview(GitHubReviewError::Capacity))
    ));
    assert_eq!(
        deliver(&host, Uuid::new_v4(), &payload('b'))
            .await
            .expect("duplicate at capacity"),
        original
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM host_review_requests", [], |row| row
            .get::<_, i64>(
            0
        ))
        .expect("count"),
        1024
    );
}
