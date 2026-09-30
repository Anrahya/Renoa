use super::*;
use crate::HostObserver;

const HOUR: i64 = 3_600_000;

async fn create(
    h: &LocalHost,
    actor: AgentId,
    agent: AgentId,
    schedule: AutomationSchedule,
) -> AutomationRecord {
    h.manage_automation(
        actor,
        Uuid::new_v4(),
        AutomationMutation::Create {
            spec: AutomationSpec {
                schedule,
                ..spec(agent)
            },
        },
        0,
        CancellationToken::new(),
    )
    .await
    .expect("automation")
}

#[test]
fn a_run_status_is_stored_under_its_serialized_name() {
    for status in RunStatus::ALL {
        assert_eq!(
            serde_json::to_value(status).expect("serialize"),
            status.as_str()
        );
    }
}

#[test]
fn a_recurring_run_may_start_half_the_gap_to_its_next_run_late_and_a_one_time_run_any_time() {
    assert_eq!(
        cron("0 */6 * * *", "UTC").skip_after_ms(0).expect("limit"),
        Some(3 * HOUR)
    );
    // Friday 09:00 to Monday 09:00 is 72 h; Monday to Tuesday is 24 h.
    let weekdays = cron("0 9 * * 1-5", "UTC");
    let friday = "2026-10-02T09:00:00Z"
        .parse::<jiff::Timestamp>()
        .expect("friday")
        .as_millisecond();
    assert_eq!(
        weekdays.skip_after_ms(friday).expect("limit"),
        Some(36 * HOUR)
    );
    assert_eq!(
        weekdays.skip_after_ms(friday + 72 * HOUR).expect("limit"),
        Some(12 * HOUR)
    );
    assert_eq!(
        AutomationSchedule::Once {
            at: "1970-01-02T00:00:00Z".to_owned()
        }
        .skip_after_ms(0)
        .expect("limit"),
        None
    );
}

#[tokio::test]
async fn a_run_later_than_its_schedule_allows_is_skipped_and_the_schedule_moves_on() {
    let (_d, h, parent, child) = fixture().await;
    let automation = create(
        &h,
        parent,
        child,
        AutomationSchedule::Cron {
            expression: "0 */6 * * *".to_owned(),
            timezone: "UTC".to_owned(),
        },
    )
    .await;
    let now = automation.next_due_ms + 3 * HOUR + 60_000;

    let skipped = runs::next(&h.config.database, now)
        .expect("admission")
        .expect("the late occurrence is recorded");
    assert_eq!(
        skipped.result,
        Some(RunResult {
            status: RunStatus::Skipped,
            output: "Scheduled run skipped: it reached the scheduler 3 h 1 min after its due time, and a run of this schedule is skipped once it is more than 3 h late.".to_owned(),
            failed_tool_calls: None,
            finished_at_ms: Some(now),
        })
    );
    assert_eq!(
        runs::next(&h.config.database, now).expect("admission"),
        None,
        "nothing is left to run until the next occurrence"
    );
    assert!(
        h.automation(child, automation.id)
            .await
            .expect("automation")
            .next_due_ms
            > now
    );
}

#[tokio::test]
async fn a_run_is_told_its_automation_and_due_time_and_how_late_it_started() {
    let (_d, h, parent, child) = fixture().await;
    let automation = create(&h, parent, child, cron("30 9 * * *", "Asia/Kolkata")).await;
    let on_time = runs::next(&h.config.database, automation.next_due_ms + 60_000)
        .expect("admission")
        .expect("run");
    assert_eq!(on_time.result, None);
    assert_eq!(
        on_time.submission,
        "(Scheduled run \"Digest\", due 09:30 IST.)\n\nscheduled digest"
    );
    runs::finish(&h.config.database, on_time.id, &succeeded("done"), 0).expect("finish");

    let next_due = h
        .automation(child, automation.id)
        .await
        .expect("automation")
        .next_due_ms;
    let late = runs::next(&h.config.database, next_due + 12 * HOUR)
        .expect("admission")
        .expect("run");
    assert_eq!(late.result, None, "exactly half the gap late still runs");
    assert_eq!(
        late.submission,
        "(Scheduled run \"Digest\", due 09:30 IST. Started 12 h late.)\n\nscheduled digest"
    );
}

#[tokio::test]
async fn a_one_time_run_runs_however_late() {
    let (_d, h, parent, child) = fixture().await;
    let automation = create(
        &h,
        parent,
        child,
        AutomationSchedule::Once {
            at: "1970-01-02T00:00:00Z".to_owned(),
        },
    )
    .await;
    let run = runs::next(&h.config.database, automation.next_due_ms + 72 * HOUR)
        .expect("admission")
        .expect("run");
    assert_eq!(run.result, None);
    assert_eq!(
        run.submission,
        "(Scheduled run \"Digest\". Started 72 h late.)\n\nscheduled digest"
    );
}

#[tokio::test]
async fn an_executed_run_records_its_status_and_failed_tool_calls() {
    let (_d, h, parent, child) = fixture().await;
    let automation = create(
        &h,
        parent,
        child,
        AutomationSchedule::Cron {
            expression: "0 */6 * * *".to_owned(),
            timezone: "UTC".to_owned(),
        },
    )
    .await;
    let run = runs::next(&h.config.database, automation.next_due_ms)
        .expect("admission")
        .expect("run");
    let failed = RunOutcome::Failed {
        reason: "Scheduled run failed: the model is unavailable".to_owned(),
        failed_tool_calls: 2,
    };
    runs::finish(&h.config.database, run.id, &failed, 42).expect("finish");
    assert!(matches!(
        runs::finish(&h.config.database, run.id, &failed, 43),
        Err(AutomationError::Conflict)
    ));

    assert_eq!(
        h.automation_result(child, run.id)
            .await
            .expect("read")
            .result,
        Some(RunResult {
            status: RunStatus::Failed,
            output: "Scheduled run failed: the model is unavailable".to_owned(),
            failed_tool_calls: Some(2),
            finished_at_ms: Some(42),
        })
    );
    let listed = h
        .automation_results(child, child, None)
        .await
        .expect("results")
        .remove(0);
    assert_eq!(listed.status, RunStatus::Failed);
    assert_eq!(listed.failed_tool_calls, Some(2));

    let observed = HostObserver::open(h.config.home.path())
        .expect("observer")
        .snapshot()
        .await
        .expect("snapshot")
        .automations
        .into_iter()
        .find(|observed| observed.id == automation.id)
        .expect("observed automation");
    assert_eq!(
        (
            observed.completed_runs,
            observed.failed_runs,
            observed.skipped_runs
        ),
        (1, 1, 0)
    );
    let last = observed.last_run.expect("last run");
    assert_eq!((last.id, last.status), (run.id, RunStatus::Failed));
    assert_eq!(
        (last.due_ms, last.finished_at_ms, last.failed_tool_calls),
        (run.due_ms, Some(42), Some(2))
    );
}

#[tokio::test]
async fn the_scheduler_heartbeat_is_observed() {
    let (_d, h, _parent, _child) = fixture().await;
    let observer = HostObserver::open(h.config.home.path()).expect("observer");
    assert!(
        observer
            .snapshot()
            .await
            .expect("snapshot")
            .automation_scheduler
            .is_none()
    );
    let scheduler = h.automation_scheduler().expect("scheduler");
    scheduler.heartbeat(1_234).await.expect("heartbeat");
    scheduler.heartbeat(5_678).await.expect("heartbeat");
    assert_eq!(
        observer
            .snapshot()
            .await
            .expect("snapshot")
            .automation_scheduler
            .expect("heartbeat")
            .heartbeat_ms,
        5_678
    );
}

#[tokio::test]
async fn a_stored_run_with_a_status_but_no_output_is_refused_as_corrupt() {
    let (_d, h, parent, child) = fixture().await;
    let automation = create(
        &h,
        parent,
        child,
        AutomationSchedule::Cron {
            expression: "0 */6 * * *".to_owned(),
            timezone: "UTC".to_owned(),
        },
    )
    .await;
    let run = runs::next(&h.config.database, automation.next_due_ms)
        .expect("admission")
        .expect("run");
    rusqlite::Connection::open(&h.config.database)
        .expect("open catalog")
        .execute(
            "UPDATE host_automation_runs SET status='succeeded' WHERE id=?1",
            [run.id.to_string()],
        )
        .expect("corrupt the run");

    let error = h
        .automation_result(child, run.id)
        .await
        .expect_err("a half-recorded run is not a result");
    assert!(error.to_string().contains("recorded together"), "{error}");
}
