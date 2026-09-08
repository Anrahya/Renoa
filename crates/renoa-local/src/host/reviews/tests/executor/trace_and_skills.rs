use super::*;

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
        .join("data/review-sessions")
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
