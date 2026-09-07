use super::*;

#[tokio::test]
async fn another_session_reads_the_exact_completed_result_through_model_tools() {
    let (d, h, parent, child) = fixture().await;
    let routine = h
        .manage_routine(
            parent,
            Uuid::new_v4(),
            RoutineMutation::Create { spec: spec(child) },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("schedule");
    let run = store::next(&h.config.database, routine.next_due_ms)
        .expect("admit")
        .expect("run");
    h.execute_routine_run(run.clone())
        .await
        .expect("automation");
    drop(h);
    let h = host(d.path());
    let workspace = h.bot_workspace(child).await.expect("workspace");
    let session = h
        .ensure_agent_session(child, &workspace, Uuid::new_v4())
        .await
        .expect("new interactive session");
    let outcome = session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text("read latest routine result")],
            Arc::new(Quiet),
        )
        .await
        .expect("read through real tools");
    assert!(
        matches!(outcome,LocalTurnOutcome::Completed {output,..} if output=="Digest saved: digest.md")
    );
    let exact = h
        .routine_result(parent, run.id)
        .await
        .expect("operator can inspect");
    assert_eq!(exact.prompt, "scheduled digest");
    assert!(h.routine_result(AgentId::new(), run.id).await.is_err());
    assert!(
        h.routine_results(AgentId::new(), child, None)
            .await
            .is_err()
    );
    let page = h.routine_results(child, child, None).await.expect("list");
    assert_eq!(page.len(), 1);
    assert!(
        h.routine_results(child, child, Some(page[0].sequence))
            .await
            .expect("older page")
            .is_empty()
    );
}
