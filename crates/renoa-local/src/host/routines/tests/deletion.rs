use super::*;

async fn change(
    h: &LocalHost,
    actor: AgentId,
    mutation: RoutineMutation,
) -> Result<RoutineRecord, LocalHostError> {
    h.manage_routine(actor, Uuid::new_v4(), mutation, 0, CancellationToken::new())
        .await
}

#[tokio::test]
async fn model_deletes_an_automation_and_can_still_read_its_previous_result() {
    let (_d, h, parent, child) = fixture().await;
    let record = change(&h, parent, RoutineMutation::Create { spec: spec(child) })
        .await
        .expect("create");
    let run = store::next(&h.config.database, record.next_due_ms)
        .expect("admit")
        .expect("run");
    h.execute_routine_run(run.clone())
        .await
        .expect("automation");
    let workspace = h.bot_workspace(child).await.expect("workspace");
    let chat = h
        .ensure_agent_session(child, &workspace, Uuid::new_v4())
        .await
        .expect("chat");
    let output = chat
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text(format!("delete routine {}", record.id))],
            Arc::new(Quiet),
        )
        .await
        .expect("delete through model");
    assert!(
        matches!(output,LocalTurnOutcome::Completed {output,..} if output=="Automation deleted")
    );
    assert!(
        h.list_routines(child, None)
            .await
            .expect("inventory")
            .is_empty()
    );
    assert!(h.routine(record.id).await.is_err());
    assert!(
        store::next(&h.config.database, record.next_due_ms + 100_000_000)
            .expect("no future occurrence")
            .is_none()
    );
    let output = chat
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text("read latest routine result")],
            Arc::new(Quiet),
        )
        .await
        .expect("read history through model");
    assert!(
        matches!(output,LocalTurnOutcome::Completed {output,..} if output=="Digest saved: digest.md")
    );
    assert_eq!(
        h.routine_result(parent, run.id)
            .await
            .expect("retained result")
            .prompt,
        "scheduled digest"
    );
}

#[tokio::test]
async fn deletion_is_idempotent_and_preserves_an_admitted_run_after_restart() {
    let (d, h, parent, child) = fixture().await;
    let create_id = Uuid::new_v4();
    let creation = RoutineMutation::Create { spec: spec(child) };
    let record = h
        .manage_routine(
            parent,
            create_id,
            creation.clone(),
            0,
            CancellationToken::new(),
        )
        .await
        .expect("create");
    let deletion = RoutineMutation::Delete {
        id: record.id,
        expected_revision: record.revision,
    };
    let run = store::next(&h.config.database, record.next_due_ms)
        .expect("admit")
        .expect("pending");
    let operation = Uuid::new_v4();
    let removed = h
        .manage_routine(
            child,
            operation,
            deletion.clone(),
            0,
            CancellationToken::new(),
        )
        .await
        .expect("delete");
    assert!(!removed.spec.enabled);
    assert_eq!(removed.revision, record.revision + 1);
    drop(h);
    let h = host(d.path());
    assert_eq!(
        h.manage_routine(child, operation, deletion, 1, CancellationToken::new())
            .await
            .expect("replay"),
        removed
    );
    h.manage_routine(parent, create_id, creation, 0, CancellationToken::new())
        .await
        .expect("original creation replay");
    assert!(
        h.list_routines(child, None)
            .await
            .expect("still deleted")
            .is_empty()
    );
    assert!(
        change(&h, child, RoutineMutation::RunNow { id: record.id })
            .await
            .is_err()
    );
    assert!(
        change(
            &h,
            child,
            RoutineMutation::Update {
                id: record.id,
                expected_revision: removed.revision,
                spec: record.spec
            }
        )
        .await
        .is_err()
    );
    let pending = store::next(&h.config.database, record.next_due_ms + 1)
        .expect("restart")
        .expect("retained run");
    assert_eq!(pending.id, run.id);
    h.execute_routine_run(pending)
        .await
        .expect("already admitted run finishes");
    assert!(
        store::next(&h.config.database, record.next_due_ms + 100_000_000)
            .expect("deleted schedule")
            .is_none()
    );
    assert!(
        h.routine_result(child, run.id)
            .await
            .expect("result retained")
            .output
            .is_some()
    );
}

#[tokio::test]
async fn schema_eighteen_upgrade_preserves_schedules_and_allows_deletion() {
    let (d, h, parent, child) = fixture().await;
    let record = change(&h, parent, RoutineMutation::Create { spec: spec(child) })
        .await
        .expect("create");
    let db = crate::host::catalog::open_verified(&h.config.database).expect("catalog");
    db.execute_batch("DROP TABLE host_routine_deletions; UPDATE host_metadata SET schema_version=18; PRAGMA user_version=18;").expect("old schema");
    drop(db);
    drop(h);
    let h = host(d.path());
    assert_eq!(h.routine(record.id).await.expect("preserved"), record);
    change(
        &h,
        child,
        RoutineMutation::Delete {
            id: record.id,
            expected_revision: record.revision,
        },
    )
    .await
    .expect("delete after migration");
    assert!(
        h.list_routines(child, None)
            .await
            .expect("deleted")
            .is_empty()
    );
}

#[tokio::test]
async fn rejected_or_cancelled_deletions_leave_the_automation_unchanged() {
    let (_d, h, parent, child) = fixture().await;
    let record = change(&h, parent, RoutineMutation::Create { spec: spec(child) })
        .await
        .expect("create");
    let deletion = RoutineMutation::Delete {
        id: record.id,
        expected_revision: record.revision,
    };
    assert!(change(&h, AgentId::new(), deletion.clone()).await.is_err());
    assert!(
        change(
            &h,
            child,
            RoutineMutation::Delete {
                id: record.id,
                expected_revision: record.revision + 1
            }
        )
        .await
        .is_err()
    );
    let stop = CancellationToken::new();
    stop.cancel();
    assert!(
        h.manage_routine(child, Uuid::new_v4(), deletion.clone(), 0, stop)
            .await
            .is_err()
    );
    assert_eq!(h.routine(record.id).await.expect("retained"), record);
}
