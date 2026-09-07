use super::*;

fn once(agent: AgentId, at: &str) -> RoutineSpec {
    RoutineSpec {
        schedule: RoutineSchedule::Once { at: at.to_owned() },
        ..spec(agent)
    }
}
async fn change(
    h: &LocalHost,
    actor: AgentId,
    mutation: RoutineMutation,
    now: i64,
) -> Result<RoutineRecord, LocalHostError> {
    h.manage_routine(
        actor,
        Uuid::new_v4(),
        mutation,
        now,
        CancellationToken::new(),
    )
    .await
}

#[tokio::test]
async fn model_creates_once_and_restart_recovers_its_only_admitted_execution() {
    let (d, h, parent, child) = fixture().await;
    let workspace = d.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace");
    let session = h
        .ensure_agent_session(parent, &workspace, Uuid::new_v4())
        .await
        .expect("session");
    session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text(format!(
                "create once {child} 2100-01-01T14:00:00+05:30"
            ))],
            Arc::new(Quiet),
        )
        .await
        .expect("model schedules once");
    let record = h
        .list_routines(child, None)
        .await
        .expect("routines")
        .remove(0);
    let expected = "2100-01-01T08:30:00Z"
        .parse::<jiff::Timestamp>()
        .expect("UTC")
        .as_millisecond();
    assert_eq!(record.next_due_ms, expected);
    assert!(
        store::next(&h.config.database, expected - 1)
            .expect("not due")
            .is_none()
    );
    let run = store::next(&h.config.database, expected + 60_000)
        .expect("late catchup")
        .expect("run");
    let disarmed = h.routine(record.id).await.expect("disarmed");
    assert!(!disarmed.spec.enabled);
    assert_eq!(disarmed.revision, record.revision + 1);
    assert!(
        change(
            &h,
            child,
            RoutineMutation::Update {
                id: record.id,
                expected_revision: record.revision,
                spec: record.spec
            },
            expected + 60_000
        )
        .await
        .is_err()
    );
    h.execute_routine_run(run.clone())
        .await
        .expect("real execution");
    let artifact = h
        .bot_workspace(child)
        .await
        .expect("bot workspace")
        .join("digest.md");
    assert_eq!(
        fs::read_to_string(&artifact).expect("artifact"),
        "# Digest\nSaved by the specialist."
    );
    let db = crate::host::catalog::open_verified(&h.config.database).expect("db");
    db.execute(
        "UPDATE host_routine_runs SET output=NULL WHERE id=?1",
        [run.id.to_string()],
    )
    .expect("lost Host receipt");
    drop(db);
    drop(session);
    drop(h);
    fs::write(&artifact, "preserve after execution").expect("marker");
    let restarted = host(d.path());
    let pending = store::next(&restarted.config.database, expected + 120_000)
        .expect("restart")
        .expect("same run");
    assert_eq!(pending.id, run.id);
    restarted
        .execute_routine_run(pending)
        .await
        .expect("kernel recovery");
    assert_eq!(
        fs::read_to_string(artifact).expect("preserved"),
        "preserve after execution"
    );
    assert!(
        store::next(&restarted.config.database, expected + 86_400_000)
            .expect("no recurrence")
            .is_none()
    );
    assert_eq!(
        restarted
            .completed_routine_runs(0)
            .await
            .expect("inbox")
            .len(),
        1
    );
}

#[tokio::test]
async fn once_rejects_ambiguous_invalid_or_elapsed_dates() {
    let (_d, h, parent, child) = fixture().await;
    for at in [
        "1970-01-01T00:00:01",
        "not a date",
        "1969-12-31T23:59:59Z",
        "1970-01-01T00:00:01Z",
    ] {
        assert!(
            change(
                &h,
                parent,
                RoutineMutation::Create {
                    spec: once(child, at)
                },
                1000
            )
            .await
            .is_err()
        );
    }
}

#[tokio::test]
async fn once_allows_pausing_and_rescheduling_and_manual_run_disarms() {
    let (_d, h, parent, child) = fixture().await;
    let record = change(
        &h,
        parent,
        RoutineMutation::Create {
            spec: once(child, "1970-01-01T00:00:02Z"),
        },
        1000,
    )
    .await
    .expect("future date");
    let mut paused = record.spec;
    paused.enabled = false;
    let paused = change(
        &h,
        child,
        RoutineMutation::Update {
            id: record.id,
            expected_revision: record.revision,
            spec: paused,
        },
        3000,
    )
    .await
    .expect("pause even after deadline");
    assert!(
        store::next(&h.config.database, 3000)
            .expect("paused")
            .is_none()
    );
    let mut rearmed = paused.spec;
    rearmed.enabled = true;
    assert!(
        change(
            &h,
            child,
            RoutineMutation::Update {
                id: record.id,
                expected_revision: paused.revision,
                spec: rearmed.clone()
            },
            3000
        )
        .await
        .is_err()
    );
    rearmed.schedule = RoutineSchedule::Once {
        at: "1970-01-01T00:00:05Z".to_owned(),
    };
    let rearmed = change(
        &h,
        child,
        RoutineMutation::Update {
            id: record.id,
            expected_revision: paused.revision,
            spec: rearmed,
        },
        3000,
    )
    .await
    .expect("reschedule");
    let op = Uuid::new_v4();
    let manual = RoutineMutation::RunNow { id: record.id };
    let receipt = h
        .manage_routine(child, op, manual.clone(), 4000, CancellationToken::new())
        .await
        .expect("run early");
    assert!(!receipt.spec.enabled);
    assert_eq!(receipt.revision, rearmed.revision + 1);
    assert_eq!(
        receipt,
        h.manage_routine(child, op, manual, 6000, CancellationToken::new())
            .await
            .expect("manual replay")
    );
    let run = store::next(&h.config.database, 6000)
        .expect("pending")
        .expect("manual");
    assert_eq!(run.id, op);
    store::finish(&h.config.database, op, "done").expect("finish");
    assert!(
        store::next(&h.config.database, 7000)
            .expect("no timed duplicate")
            .is_none()
    );
}

#[tokio::test]
async fn schema_seventeen_upgrade_retains_existing_routines_and_receipts() {
    let (d, h, parent, child) = fixture().await;
    let record = change(&h, parent, RoutineMutation::Create { spec: spec(child) }, 0)
        .await
        .expect("interval");
    let db = crate::host::catalog::open_verified(&h.config.database).expect("db");
    db.execute_batch("UPDATE host_metadata SET schema_version=17; PRAGMA user_version=17;")
        .expect("old schema");
    drop(db);
    drop(h);
    let restored = host(d.path());
    assert_eq!(
        restored.routine(record.id).await.expect("preserved"),
        record
    );
    let replay = restored
        .manage_routine(
            parent,
            record.id,
            RoutineMutation::Create { spec: record.spec },
            9999,
            CancellationToken::new(),
        )
        .await
        .expect("receipt preserved");
    assert_eq!(replay.next_due_ms, record.next_due_ms);
}
