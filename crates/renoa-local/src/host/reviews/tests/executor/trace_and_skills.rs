use super::*;

#[tokio::test]
async fn a_git_review_loads_another_skill_through_plugins_and_reattaches_it_once_for_validation() {
    let (directory, host, id, api) = prepared("git-skills").await;
    fs::write(
        directory.path().join("git_model.mjs"),
        include_str!("../git_model.mjs"),
    )
    .unwrap();
    for (name, guidance) in [
        ("renoa-code-review", "Frozen review guidance"),
        ("extra-review", "Extra pinned review guidance"),
    ] {
        let skill = directory.path().join("skills").join(name);
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Review guidance\n---\n{guidance}\n"),
        )
        .unwrap();
    }
    let (repo, _, base, head) = crate::git_repository::tests::fixture();
    api.state.lock().unwrap().commits = Some((base, head));
    let result = host
        .execute_git_at(id, repo.path(), api.origin.clone())
        .await
        .unwrap();
    if let GitHubReviewRun::Finished {
        outcome: GitHubReviewOutcome::Incomplete { reason },
        ..
    } = &result
    {
        panic!(
            "{reason}: {}",
            fs::read_to_string(directory.path().join("auth.sqlite.error")).unwrap_or_default()
        );
    }
    assert!(
        matches!(
            result,
            GitHubReviewRun::Finished {
                outcome: GitHubReviewOutcome::Reviewed { .. },
                ..
            }
        ),
        "review must complete"
    );
    api.stop().await;
}

#[tokio::test]
async fn review_freezes_a_shared_skill_and_records_both_stages_without_retracing_replay() {
    let (directory, first, id, api) = prepared("").await;
    let skill = directory.path().join("skills/renoa-code-review");
    fs::create_dir_all(&skill).expect("skill directory");
    let path = skill.join("SKILL.md");
    fs::write(&path, "---\nname: renoa-code-review\ndescription: Review fixture\n---\nFrozen review guidance v1\n").expect("skill");
    let result = execute(&first, id, &api).await.expect("review");
    let GitHubReviewRun::Finished {
        snapshot: Some(snapshot),
        ..
    } = &result
    else {
        panic!("review snapshot missing")
    };
    assert!(snapshot.system_prompt.contains("Frozen review guidance v1"));
    assert!(snapshot.system_prompt.contains("skill:renoa-code-review:"));
    let trace_path = directory
        .path()
        .join("data/state/review-sessions")
        .join(id.to_string())
        .join(crate::trace::TRACE_DATABASE);
    let trace = rusqlite::Connection::open(&trace_path).expect("trace");
    let count = |sql: &str| {
        trace
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .expect("count")
    };
    assert_eq!(
        count(
            "SELECT count(*) FROM runs WHERE status='completed' AND trace_complete=1 AND duration_us IS NOT NULL"
        ),
        2
    );
    assert_eq!(
        count(
            "SELECT count(*) FROM events WHERE component='model' AND kind='request_finished' AND duration_us IS NOT NULL AND cache_read_tokens IS NOT NULL"
        ),
        4
    );
    assert!(
        count(
            "SELECT count(*) FROM events WHERE component='tool' AND kind='execution_finished' AND duration_us IS NOT NULL"
        ) > 0
    );
    let events = count("SELECT count(*) FROM events");
    drop(first);
    fs::write(
        &path,
        "---\nname: renoa-code-review\ndescription: Review fixture\n---\nChanged guidance v2\n",
    )
    .expect("update shared skill");
    assert_eq!(
        execute(&host(directory.path()), id, &api)
            .await
            .expect("replay"),
        result
    );
    assert_eq!(count("SELECT count(*) FROM events"), events);
    api.stop().await;
}
