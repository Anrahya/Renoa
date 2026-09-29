use super::*;

const MINUTE: i64 = 60_000;

async fn create(h: &LocalHost, actor: AgentId, spec: AutomationSpec) -> (Uuid, AutomationRecord) {
    let operation = Uuid::new_v4();
    let record = h
        .manage_automation(
            actor,
            operation,
            AutomationMutation::Create { spec },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("automation");
    (operation, record)
}

fn runs_of(h: &LocalHost, automation: Uuid) -> Vec<(i64, bool)> {
    rusqlite::Connection::open(&h.config.database)
        .expect("catalog")
        .prepare(
            "SELECT admitted_at_ms, output IS NULL FROM host_automation_runs
             WHERE automation_id=?1 ORDER BY sequence",
        )
        .expect("prepare")
        .query_map([automation.to_string()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("runs")
}

/// Every text value anywhere in the catalog that contains `needle`.
fn holding(h: &LocalHost, needle: &str) -> Vec<String> {
    let db = rusqlite::Connection::open(&h.config.database).expect("catalog");
    let tables = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .expect("prepare")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("tables")
        .collect::<Result<Vec<_>, _>>()
        .expect("tables");
    let mut found = Vec::new();
    for table in tables {
        let columns = db
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .expect("prepare")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("columns")
            .collect::<Result<Vec<_>, _>>()
            .expect("columns");
        for column in columns {
            let hits: i64 = db
                .query_row(
                    &format!("SELECT count(*) FROM \"{table}\" WHERE instr(CAST(\"{column}\" AS TEXT), ?1) > 0"),
                    [needle],
                    |row| row.get(0),
                )
                .expect("search");
            if hits > 0 {
                found.push(format!("{table}.{column}"));
            }
        }
    }
    found
}

#[tokio::test]
async fn an_automation_keeps_its_newest_fifty_finished_runs() {
    let (_d, h, parent, child) = fixture().await;
    let (_, automation) = create(
        &h,
        parent,
        AutomationSpec {
            schedule: cron("*/5 * * * *", "UTC"),
            ..spec(child)
        },
    )
    .await;
    let mut now = automation.next_due_ms;
    for _ in 0..52 {
        let run = runs::next(&h.config.database, now)
            .expect("admission")
            .expect("run");
        runs::finish(&h.config.database, run.id, &succeeded("done"), now).expect("finish");
        now += 5 * MINUTE;
    }
    let kept = runs_of(&h, automation.id);
    assert_eq!(kept.len(), 50);
    assert_eq!(
        kept[0].0,
        automation.next_due_ms + 2 * 5 * MINUTE,
        "the two oldest runs went"
    );
}

#[tokio::test]
async fn finished_runs_expire_after_thirty_days_and_unfinished_ones_stay() {
    let (_d, h, parent, child) = fixture().await;
    let (_, automation) = create(&h, parent, spec(child)).await;
    let first = runs::next(&h.config.database, automation.next_due_ms)
        .expect("admission")
        .expect("run");
    runs::finish(
        &h.config.database,
        first.id,
        &succeeded("done"),
        first.due_ms,
    )
    .expect("finish");
    let second = runs::next(&h.config.database, automation.next_due_ms + 12 * 3_600_000)
        .expect("admission")
        .expect("run");
    let scheduler = h.automation_scheduler().expect("scheduler");

    let thirty_days = 30 * 24 * 3_600_000;
    scheduler
        .heartbeat(first.due_ms + thirty_days)
        .await
        .expect("heartbeat");
    assert_eq!(
        runs_of(&h, automation.id).len(),
        2,
        "exactly thirty days stays"
    );
    scheduler
        .heartbeat(second.admitted_at_ms + thirty_days + 1)
        .await
        .expect("heartbeat");
    assert_eq!(
        runs_of(&h, automation.id),
        [(second.admitted_at_ms, true)],
        "an unfinished run is never expired, however old"
    );
}

#[tokio::test]
async fn a_deleted_automation_leaves_no_trace_of_its_content() {
    let (_d, h, parent, child) = fixture().await;
    let secret = "Summarize my private notes";
    let (create_op, automation) = create(
        &h,
        parent,
        AutomationSpec {
            name: "Private digest".to_owned(),
            prompt: secret.to_owned(),
            ..spec(child)
        },
    )
    .await;
    let run = runs::next(&h.config.database, automation.next_due_ms)
        .expect("admission")
        .expect("run");
    let delete_op = Uuid::new_v4();
    let delete = AutomationMutation::Delete {
        id: automation.id,
        expected_revision: automation.revision,
    };
    h.manage_automation(
        parent,
        delete_op,
        delete.clone(),
        1,
        CancellationToken::new(),
    )
    .await
    .expect("delete");
    assert!(
        !holding(&h, secret).is_empty(),
        "the unfinished run keeps its task until it finishes"
    );

    let scheduler = h.automation_scheduler().expect("scheduler");
    assert_eq!(
        scheduler.conversations_to_delete().await.expect("marks"),
        Vec::<Uuid>::new(),
        "an automation with a run in flight keeps its conversation"
    );
    runs::finish(&h.config.database, run.id, &succeeded("private answer"), 2).expect("finish");
    assert_eq!(
        scheduler.conversations_to_delete().await.expect("marks"),
        [automation.id],
        "its conversation is left for the schedule's owner to delete"
    );
    scheduler
        .conversation_deleted(automation.id)
        .await
        .expect("record the deletion");
    assert!(
        scheduler
            .conversations_to_delete()
            .await
            .expect("marks")
            .is_empty()
    );
    assert_eq!(holding(&h, secret), Vec::<String>::new());
    assert_eq!(holding(&h, "Private digest"), Vec::<String>::new());
    assert_eq!(holding(&h, "private answer"), Vec::<String>::new());
    assert!(runs_of(&h, automation.id).is_empty());

    for (operation, mutation) in [
        (
            create_op,
            AutomationMutation::Create {
                spec: AutomationSpec {
                    name: "Private digest".to_owned(),
                    prompt: secret.to_owned(),
                    ..spec(child)
                },
            },
        ),
        (delete_op, delete),
    ] {
        let replayed = h
            .manage_automation(parent, operation, mutation, 3, CancellationToken::new())
            .await
            .expect("a retried operation still gets its answer");
        assert_eq!(replayed.id, automation.id);
        assert_eq!(
            (replayed.spec.name.as_str(), replayed.spec.prompt.as_str()),
            ("", "")
        );
    }
    assert!(
        h.automation(child, automation.id).await.is_err(),
        "and cannot bring the automation back"
    );
    assert_eq!(holding(&h, secret), Vec::<String>::new());
}

#[tokio::test]
async fn deleting_an_idle_automation_removes_its_data_at_once() {
    let (_d, h, parent, child) = fixture().await;
    let (_, automation) = create(&h, parent, spec(child)).await;
    let run = runs::next(&h.config.database, automation.next_due_ms)
        .expect("admission")
        .expect("run");
    runs::finish(&h.config.database, run.id, &succeeded("done"), 1).expect("finish");
    h.manage_automation(
        parent,
        Uuid::new_v4(),
        AutomationMutation::Delete {
            id: automation.id,
            expected_revision: automation.revision,
        },
        2,
        CancellationToken::new(),
    )
    .await
    .expect("delete");
    assert!(runs_of(&h, automation.id).is_empty());
    assert_eq!(holding(&h, "scheduled digest"), Vec::<String>::new());
}
