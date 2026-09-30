use super::*;

#[tokio::test]
async fn another_session_reads_the_exact_completed_result_through_model_tools() {
    let (d, h, parent, child) = fixture().await;
    let automation = h
        .manage_automation(
            parent,
            Uuid::new_v4(),
            AutomationMutation::Create { spec: spec(child) },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("schedule");
    let run = runs::next(&h.config.database, automation.next_due_ms)
        .expect("admit")
        .expect("run");
    runs::finish(
        &h.config.database,
        run.id,
        &succeeded("Digest saved: digest.md"),
        0,
    )
    .expect("result");
    drop(h);
    let h = host(d.path());
    let workspace = h.agent_workspace(child).await.expect("workspace");
    let session = h
        .ensure_agent_session(child, &workspace, Uuid::new_v4())
        .await
        .expect("new interactive session");
    let outcome = session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text("read latest automation result")],
            Arc::new(Quiet),
        )
        .await
        .expect("read through real tools");
    assert!(
        matches!(outcome,LocalTurnOutcome::Completed {output,..} if output=="Digest saved: digest.md")
    );
    let exact = h
        .automation_result(parent, run.id)
        .await
        .expect("operator can inspect");
    assert!(exact.submission.ends_with("\n\nscheduled digest"));
    let denied = outsider(&h).await;
    assert!(h.automation_result(denied, run.id).await.is_err());
    assert!(h.automation_results(denied, child, None).await.is_err());
    let page = h
        .automation_results(child, child, None)
        .await
        .expect("list");
    assert_eq!(page.len(), 1);
    assert!(
        h.automation_results(child, child, Some(page[0].sequence))
            .await
            .expect("older page")
            .is_empty()
    );
}
