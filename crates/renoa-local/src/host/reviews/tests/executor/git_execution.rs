use super::*;

#[tokio::test]
async fn real_git_review_validates_late_and_deleted_findings_then_publishes_once() {
    let (directory, host, id, api) = prepared("git").await;
    fs::write(
        directory.path().join("git_model.mjs"),
        include_str!("../git_model.mjs"),
    )
    .expect("Git fixture model");
    let (repo_dir, _repository, base, head) = crate::git_repository::tests::fixture();
    api.state.lock().expect("state").commits = Some((base.clone(), head.clone()));
    api.state.lock().expect("state").changed_files = Some(605);
    let result = host
        .execute_git_at(id, repo_dir.path(), api.origin.clone())
        .await
        .expect("Git review");
    let GitHubReviewRun::Finished {
        snapshot: Some(snapshot),
        outcome: GitHubReviewOutcome::Reviewed { report, .. },
        ..
    } = &result
    else {
        panic!("reviewed: {result:?}")
    };
    assert_eq!(snapshot.context.files.len(), 605);
    assert!(
        snapshot
            .context
            .files
            .iter()
            .all(|file| file.patch.is_none())
    );
    assert!(
        serde_json::to_string(&snapshot.context.prompt().expect("prompt"))
            .expect("prompt JSON")
            .len()
            < 4_000
    );
    assert_eq!(report.findings.len(), 3);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.path == "z-bug.rs" && f.in_diff)
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.path == "removed.rs" && f.side == crate::GitSide::Base && f.in_diff)
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.path == "caller.rs" && !f.in_diff)
    );
    assert!(report.limitations.iter().all(|s| !s.contains("Rejected")));
    assert!(
        report
            .limitations
            .iter()
            .all(|s| !s.contains("Coverage is partial"))
    );
    assert_inventory_accounting(&snapshot.context, &head);
    let first = publication::publish(&host, id, &api)
        .await
        .expect("publish");
    assert_publication(&api);
    assert_eq!(
        first,
        publication::publish(&host, id, &api)
            .await
            .expect("idempotent replay")
    );
    let calls = fs::read_to_string(directory.path().join("auth.sqlite.calls")).expect("calls");
    assert_eq!(
        result,
        host.execute_git_at(id, repo_dir.path(), api.origin.clone())
            .await
            .expect("completed replay")
    );
    assert_eq!(
        calls,
        fs::read_to_string(directory.path().join("auth.sqlite.calls")).expect("no new calls")
    );
    api.stop().await;
}

fn assert_publication(api: &Api) {
    let state = api.state.lock().expect("state");
    let body: serde_json::Value = serde_json::from_str(
        state
            .bodies
            .iter()
            .find(|body| body.contains("\"event\":\"COMMENT\""))
            .expect("publication body"),
    )
    .expect("JSON");
    assert_eq!(body["comments"].as_array().expect("comments").len(), 2);
    assert!(
        body["comments"]
            .as_array()
            .expect("comments")
            .iter()
            .any(|c| c["path"] == "removed.rs" && c["side"] == "LEFT")
    );
    assert!(
        body["body"]
            .as_str()
            .expect("body")
            .contains("caller.rs:33")
    );
    assert_eq!(state.reviews.len(), 1);
}

fn assert_inventory_accounting(context: &ReviewContext, head: &str) {
    let page = renoa_agent::ToolResult {
        call_id: "page".to_owned(), name: "git_changes".to_owned(), is_error: false, details: None,
        content: vec![renoa_agent::ContentBlock::text(serde_json::json!({"base":context.merge_base_sha,"head":head,"changes":[{"path":"z-bug.rs"}]}).to_string())],
    };
    let gap = context
        .inventory_limitation(head, [&page, &page].into_iter())
        .expect("partial inventory");
    assert!(
        gap.contains("1 of 605"),
        "repeated pages cannot inflate coverage: {gap}"
    );
    let wrong = context
        .inventory_limitation("different-commit", [&page].into_iter())
        .expect("wrong comparison");
    assert!(wrong.contains("0 of 605"));
}
