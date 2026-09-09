use super::*;
use crate::host::catalog;

fn command(revision: i64, enabled: bool) -> RoutineEnablement {
    RoutineEnablement {
        operation_id: Uuid::new_v4(),
        expected_revision: revision,
        enabled,
    }
}

async fn controls(h: &LocalHost) -> (HostRoutineControl, Uuid) {
    let owner = Uuid::new_v4();
    let control = HostRoutineControl::open(
        h.config.database.parent().expect("root"),
        h.host_id().await.expect("Host"),
        owner,
    )
    .expect("owner controls");
    (control, owner)
}

async fn create(h: &LocalHost, parent: AgentId, spec: RoutineSpec) -> RoutineRecord {
    h.manage_routine(
        parent,
        Uuid::new_v4(),
        RoutineMutation::Create { spec },
        0,
        CancellationToken::new(),
    )
    .await
    .expect("routine")
}

#[tokio::test]
async fn owner_pause_keeps_admitted_work_and_restart_replays_receipt_after_an_agent_edit() {
    let (d, h, parent, child) = fixture().await;
    let routine = create(&h, parent, spec(child)).await;
    let admitted = store::next(&h.config.database, routine.next_due_ms)
        .expect("admission")
        .expect("run");
    let (control, owner) = controls(&h).await;
    let pause = command(1, false);
    let paused = control
        .set_enabled(owner, routine.id, pause.clone(), routine.next_due_ms + 1)
        .await
        .expect("pause");
    assert!(!paused.spec.enabled);
    assert_eq!(paused.revision, 2);
    assert_eq!(
        store::next(&h.config.database, 100_000_000)
            .expect("pending")
            .expect("admitted run retained"),
        admitted
    );
    store::finish(&h.config.database, admitted.id, "completed while paused").expect("finish");
    assert!(
        store::next(&h.config.database, 200_000_000)
            .expect("paused admission")
            .is_none()
    );

    let mut changed = paused.spec.clone();
    changed.prompt = "new standing instructions".to_owned();
    let newer = h
        .manage_routine(
            child,
            Uuid::new_v4(),
            RoutineMutation::Update {
                id: routine.id,
                expected_revision: 2,
                spec: changed,
            },
            200_000_000,
            CancellationToken::new(),
        )
        .await
        .expect("agent updates same record");
    let id = h.host_id().await.expect("Host");
    drop(control);
    drop(h);
    let control = HostRoutineControl::open(&d.path().join("data"), id, owner)
        .expect("restart without runtime");
    assert_eq!(
        control
            .set_enabled(owner, routine.id, pause, 300_000_000)
            .await
            .expect("recover lost response"),
        paused
    );
    let resumed = control
        .set_enabled(
            owner,
            routine.id,
            command(newer.revision, true),
            300_000_000,
        )
        .await
        .expect("resume");
    assert!(resumed.spec.enabled);
    assert_eq!(resumed.spec.prompt, "new standing instructions");
    assert_eq!(resumed.next_due_ms, 343_200_000);
    let db = catalog::open_verified(&d.path().join("data/host.sqlite3")).expect("shared catalog");
    assert_eq!(store::get(&db, routine.id).expect("latest"), resumed);
}

#[tokio::test]
async fn owner_and_agent_authority_are_distinct_and_old_revisions_do_not_overwrite() {
    let (_d, h, parent, child) = fixture().await;
    let routine = create(&h, parent, spec(child)).await;
    let (control, owner) = controls(&h).await;
    let pause = command(1, false);
    assert!(matches!(
        control
            .set_enabled(Uuid::new_v4(), routine.id, pause.clone(), 1)
            .await,
        Err(LocalHostError::Routine(RoutineError::Forbidden))
    ));
    assert!(
        h.manage_routine(
            AgentId::from_uuid(owner),
            pause.operation_id,
            RoutineMutation::SetEnabled {
                id: routine.id,
                expected_revision: 1,
                enabled: false
            },
            1,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    let paused = control
        .set_enabled(owner, routine.id, pause.clone(), 1)
        .await
        .expect("real owner");
    assert!(matches!(
        control
            .set_enabled(owner, routine.id, command(1, true), 2)
            .await,
        Err(LocalHostError::Routine(RoutineError::Conflict))
    ));
    let different = RoutineEnablement {
        enabled: true,
        ..pause
    };
    assert!(matches!(
        control.set_enabled(owner, routine.id, different, 2).await,
        Err(LocalHostError::Routine(RoutineError::Conflict))
    ));
    assert_eq!(
        h.routine(routine.id)
            .await
            .expect("same authoritative state"),
        paused
    );
}

#[tokio::test]
async fn concurrent_owner_retries_commit_once_and_competing_edits_conflict() {
    let (_d, h, parent, child) = fixture().await;
    let routine = create(&h, parent, spec(child)).await;
    let (control, owner) = controls(&h).await;
    let pause = command(1, false);
    let (a, b) = tokio::join!(
        control.set_enabled(owner, routine.id, pause.clone(), 1),
        control.set_enabled(owner, routine.id, pause, 2)
    );
    assert_eq!(a.expect("first"), b.expect("duplicate"));
    let (a, b) = tokio::join!(
        control.set_enabled(owner, routine.id, command(2, true), 3),
        control.set_enabled(owner, routine.id, command(2, false), 4)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let error = a.err().or_else(|| b.err()).expect("one conflict");
    assert!(matches!(
        error,
        LocalHostError::Routine(RoutineError::Conflict)
    ));
    let db = catalog::open_verified(&h.config.database).expect("catalog");
    let receipts: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM host_routine_owner_mutations",
            [],
            |r| r.get(0),
        )
        .expect("receipts");
    assert_eq!(receipts, 2);
    assert_eq!(h.routine(routine.id).await.expect("revision").revision, 3);
}

#[tokio::test]
async fn owner_receipt_failure_rolls_back_change_and_migration_keeps_agent_receipts() {
    let (d, h, parent, child) = fixture().await;
    let op = Uuid::new_v4();
    let creation = RoutineMutation::Create { spec: spec(child) };
    let routine = h
        .manage_routine(parent, op, creation.clone(), 0, CancellationToken::new())
        .await
        .expect("create");
    let db = catalog::open_verified(&h.config.database).expect("catalog");
    db.execute_batch("DROP TABLE host_routine_owner_mutations; UPDATE host_metadata SET schema_version=23; PRAGMA user_version=23;").expect("schema 23");
    drop(db);
    drop(h);
    let h = host(d.path());
    assert_eq!(
        h.manage_routine(parent, op, creation, 100, CancellationToken::new())
            .await
            .expect("old receipt after migration"),
        routine
    );
    let (control, owner) = controls(&h).await;
    let pause = command(1, false);
    let db = catalog::open_verified(&h.config.database).expect("catalog");
    db.execute_batch("CREATE TRIGGER reject_owner_receipt BEFORE INSERT ON host_routine_owner_mutations BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").expect("storage failure boundary");
    assert!(
        control
            .set_enabled(owner, routine.id, pause.clone(), 100)
            .await
            .is_err()
    );
    assert_eq!(h.routine(routine.id).await.expect("rolled back"), routine);
    db.execute_batch("DROP TRIGGER reject_owner_receipt")
        .expect("restore storage");
    assert_eq!(
        control
            .set_enabled(owner, routine.id, pause, 200)
            .await
            .expect("retry")
            .revision,
        2
    );
}

#[tokio::test]
async fn expired_once_deleted_routines_and_replaced_hosts_cannot_be_resumed() {
    let (_d, h, parent, child) = fixture().await;
    let mut once = spec(child);
    once.schedule = RoutineSchedule::Once {
        at: "1970-01-01T00:00:01Z".to_owned(),
    };
    let routine = create(&h, parent, once).await;
    let (control, owner) = controls(&h).await;
    let pause = command(1, false);
    control
        .set_enabled(owner, routine.id, pause.clone(), 2_000)
        .await
        .expect("pause overdue once");
    assert!(matches!(
        control
            .set_enabled(owner, routine.id, command(2, true), 2_000)
            .await,
        Err(LocalHostError::Routine(RoutineError::Invalid(_)))
    ));
    h.manage_routine(
        child,
        Uuid::new_v4(),
        RoutineMutation::Delete {
            id: routine.id,
            expected_revision: 2,
        },
        2_000,
        CancellationToken::new(),
    )
    .await
    .expect("delete");
    assert!(matches!(
        control
            .set_enabled(owner, routine.id, command(3, true), 2_000)
            .await,
        Err(LocalHostError::Routine(RoutineError::NotFound))
    ));
    // A receipt may be read after deletion, but cannot resurrect the routine.
    assert_eq!(
        control
            .set_enabled(owner, routine.id, pause.clone(), 3_000)
            .await
            .expect("historical receipt")
            .revision,
        2
    );
    assert!(h.routine(routine.id).await.is_err());
    let db = catalog::open_verified(&h.config.database).expect("catalog");
    db.execute(
        "UPDATE host_identity SET host_id=?1",
        [Uuid::new_v4().to_string()],
    )
    .expect("replace Host identity");
    assert!(
        control
            .set_enabled(owner, routine.id, pause, 4_000)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_missing_catalog_is_not_recreated_by_an_owner_write() {
    let (_d, h, parent, child) = fixture().await;
    let routine = create(&h, parent, spec(child)).await;
    let (control, owner) = controls(&h).await;
    std::fs::remove_file(&h.config.database).expect("catalog removed during outage");
    assert!(
        control
            .set_enabled(owner, routine.id, command(1, false), 1)
            .await
            .is_err()
    );
    assert!(!h.config.database.exists());
}
