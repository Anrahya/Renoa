//! What an agent's automation runs write to the owner's feed, in the
//! transaction that records them.

use super::*;
use crate::{FeedItem, FeedKind, HostFeed};

const HOUR: i64 = 3_600_000;

async fn create(h: &LocalHost, child: AgentId, name: &str) -> AutomationRecord {
    h.manage_automation(
        child,
        Uuid::new_v4(),
        AutomationMutation::Create {
            spec: AutomationSpec {
                name: name.to_owned(),
                schedule: cron("0 */6 * * *", "UTC"),
                ..spec(child)
            },
        },
        0,
        CancellationToken::new(),
    )
    .await
    .expect("automation")
}

/// Admits a run of `automation` now and returns its identity.
async fn admit(h: &LocalHost, child: AgentId, automation: &AutomationRecord) -> Uuid {
    let operation = Uuid::new_v4();
    h.manage_automation(
        child,
        operation,
        AutomationMutation::RunNow { id: automation.id },
        0,
        CancellationToken::new(),
    )
    .await
    .expect("run now");
    operation
}

/// Admits a run of `automation` and ends it with `outcome` at `now`.
async fn ran(
    h: &LocalHost,
    child: AgentId,
    automation: &AutomationRecord,
    outcome: &RunOutcome,
    now: i64,
) -> Uuid {
    let run = admit(h, child, automation).await;
    runs::finish(&h.config.database, run, outcome, now).expect("finish");
    run
}

fn failed(reason: &str) -> RunOutcome {
    RunOutcome::Failed {
        reason: reason.to_owned(),
        failed_tool_calls: 0,
    }
}

fn in_conversation(answer: &str) -> RunOutcome {
    RunOutcome::Succeeded {
        answer: answer.to_owned(),
        failed_tool_calls: 0,
        landing: RunLanding::Conversation,
    }
}

async fn items(h: &LocalHost) -> Vec<FeedItem> {
    HostFeed::open(h.config.home.path())
        .expect("feed")
        .list(None)
        .await
        .expect("list")
        .items
}

/// The kind, subject and doneness of every item, oldest first.
async fn shape(h: &LocalHost) -> Vec<(FeedKind, String, bool)> {
    let mut items = items(h).await;
    items.reverse();
    items
        .into_iter()
        .map(|item| (item.kind, item.subject_id, item.done))
        .collect()
}

#[tokio::test]
async fn a_failed_a_skipped_and_a_refused_run_each_open_one_problem() {
    let (_d, h, _parent, child) = fixture().await;
    // The only automation due when the scheduler next looks, hours late.
    let late = create(&h, child, "Late").await;
    let skipped = runs::next(
        &h.config.database,
        late.next_due_ms + 3 * HOUR + 60_000,
        false,
    )
    .expect("admission")
    .expect("the late occurrence is recorded");
    assert_eq!(
        (
            skipped.automation_id,
            skipped.result.map(|result| result.status)
        ),
        (late.id, Some(RunStatus::Skipped))
    );
    let failing = create(&h, child, "Failing").await;
    ran(
        &h,
        child,
        &failing,
        &failed("Scheduled run failed: down"),
        1,
    )
    .await;
    let refused = create(&h, child, "Refused").await;
    let message = "Scheduled run could not be sent: agent no longer exists";
    ran(&h, child, &refused, &failed(message), 2).await;

    let found = items(&h).await;
    assert_eq!(
        found
            .iter()
            .rev()
            .map(|item| (item.kind, item.subject_id.clone(), item.title.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                FeedKind::Problem,
                late.id.to_string(),
                "\u{201c}Late\u{201d} was skipped"
            ),
            (
                FeedKind::Problem,
                failing.id.to_string(),
                "\u{201c}Failing\u{201d} didn\u{2019}t finish"
            ),
            (
                FeedKind::Problem,
                refused.id.to_string(),
                "\u{201c}Refused\u{201d} didn\u{2019}t finish"
            ),
        ]
    );
    assert!(
        found
            .iter()
            .all(|item| item.action == crate::FeedAction::Retry
                && item.agent.id == child
                && !item.done)
    );
}

#[tokio::test]
async fn a_failure_streak_opens_one_problem_and_a_success_closes_it() {
    let (_d, h, _parent, child) = fixture().await;
    let automation = create(&h, child, "Digest").await;
    let subject = automation.id.to_string();
    ran(
        &h,
        child,
        &automation,
        &failed("Scheduled run failed: one"),
        1,
    )
    .await;
    let second = ran(
        &h,
        child,
        &automation,
        &failed("Scheduled run failed: two"),
        2,
    )
    .await;
    assert_eq!(
        shape(&h).await,
        [(FeedKind::Problem, subject.clone(), false)],
        "the second failure is the same streak"
    );
    assert!(matches!(
        runs::finish(
            &h.config.database,
            second,
            &failed("Scheduled run failed: two"),
            3
        ),
        Err(AutomationError::Conflict)
    ));
    assert_eq!(shape(&h).await.len(), 1, "a replayed finish adds nothing");

    ran(&h, child, &automation, &in_conversation("Done."), 4).await;
    assert_eq!(
        shape(&h).await,
        [(FeedKind::Problem, subject.clone(), true)],
        "a success closes the problem and, in a conversation, adds no result"
    );
    ran(
        &h,
        child,
        &automation,
        &failed("Scheduled run failed: three"),
        5,
    )
    .await;
    assert_eq!(
        shape(&h).await,
        [
            (FeedKind::Problem, subject.clone(), true),
            (FeedKind::Problem, subject, false)
        ],
        "a failure after a success starts a new streak"
    );
}

#[tokio::test]
async fn a_result_in_its_own_task_replaces_the_last_and_one_in_a_conversation_makes_none() {
    let (_d, h, _parent, child) = fixture().await;
    let automation = create(&h, child, "Digest").await;
    let first = ran(&h, child, &automation, &succeeded("First."), 1).await;
    let found = items(&h).await;
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].kind, found[0].subject_id.clone(), found[0].action),
        (FeedKind::Result, first.to_string(), crate::FeedAction::Open)
    );
    assert_eq!(found[0].title, "\u{201c}Digest\u{201d} finished");
    let feed = HostFeed::open(h.config.home.path()).expect("feed");
    feed.see(1).await.expect("see the first result");

    let second = ran(&h, child, &automation, &succeeded("Second."), 2).await;
    let page = feed.list(None).await.expect("list");
    assert_eq!(
        page.items
            .iter()
            .map(|item| (item.kind, item.subject_id.clone()))
            .collect::<Vec<_>>(),
        [(FeedKind::Result, second.to_string())]
    );
    assert!(page.items[0].sequence > found[0].sequence);
    assert_eq!(page.unread, 1, "the replacement is unread again");

    ran(&h, child, &automation, &in_conversation("Third."), 3).await;
    assert_eq!(
        shape(&h).await,
        [(FeedKind::Result, second.to_string(), false)]
    );
}

#[tokio::test]
async fn a_problem_holds_none_of_its_error_text() {
    let (_d, h, _parent, child) = fixture().await;
    let automation = create(&h, child, "Digest").await;
    let digest = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    let error = format!(
        "Scheduled run failed: tool web_fetch could not reach https://api.example.com/v1/x for plugin {digest} on model grok-4-fast"
    );
    ran(&h, child, &automation, &failed(&error), 1).await;
    let found = items(&h).await;
    assert_eq!(found.len(), 1);
    let text = format!("{} {}", found[0].title, found[0].body);
    for leaked in [
        "web_fetch",
        "https://",
        "api.example.com",
        digest,
        "grok",
        "failed:",
    ] {
        assert!(!text.contains(leaked), "{leaked} reached the item: {text}");
    }
}

#[tokio::test]
async fn an_item_commits_with_the_run_it_reports_or_neither_does() {
    let (_d, h, _parent, child) = fixture().await;
    let automation = create(&h, child, "Digest").await;
    let run = admit(&h, child, &automation).await;
    let db = rusqlite::Connection::open(&h.config.database).expect("catalog");
    db.execute_batch(
        "CREATE TRIGGER reject_feed_item BEFORE INSERT ON host_feed_items
         BEGIN SELECT RAISE(ABORT, 'injected feed failure'); END;",
    )
    .expect("storage failure boundary");
    let outcome = failed("Scheduled run failed: down");
    assert!(runs::finish(&h.config.database, run, &outcome, 1).is_err());
    let pending = runs::next(&h.config.database, 1, false)
        .expect("pending")
        .expect("the run stays admitted");
    assert_eq!((pending.id, pending.result), (run, None));

    db.execute_batch("DROP TRIGGER reject_feed_item;")
        .expect("storage recovers");
    runs::finish(&h.config.database, run, &outcome, 2).expect("finish");
    assert_eq!(
        shape(&h).await,
        [(FeedKind::Problem, automation.id.to_string(), false)]
    );
}

#[tokio::test]
async fn deleting_an_automation_removes_its_items() {
    let (_d, h, _parent, child) = fixture().await;
    let automation = create(&h, child, "Secret project").await;
    ran(&h, child, &automation, &succeeded("Done."), 1).await;
    ran(
        &h,
        child,
        &automation,
        &failed("Scheduled run failed: down"),
        2,
    )
    .await;
    let kept = create(&h, child, "Kept").await;
    ran(&h, child, &kept, &failed("Scheduled run failed: down"), 3).await;
    assert_eq!(items(&h).await.len(), 3);
    let current = h.automation(child, automation.id).await.expect("read");
    h.manage_automation(
        child,
        Uuid::new_v4(),
        AutomationMutation::Delete {
            id: automation.id,
            expected_revision: current.revision,
        },
        4,
        CancellationToken::new(),
    )
    .await
    .expect("delete");
    assert_eq!(
        shape(&h).await,
        [(FeedKind::Problem, kept.id.to_string(), false)]
    );
    let titles: i64 = rusqlite::Connection::open(&h.config.database)
        .expect("catalog")
        .query_row(
            "SELECT count(*) FROM host_feed_items WHERE title LIKE '%Secret project%'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(titles, 0);
}

#[tokio::test]
async fn a_result_goes_with_its_run() {
    let (_d, h, _parent, child) = fixture().await;
    let automation = create(&h, child, "Digest").await;
    let first = ran(&h, child, &automation, &succeeded("Done."), 0).await;
    for now in 1..=crate::host::automations::retention::KEEP_RUNS {
        ran(
            &h,
            child,
            &automation,
            &failed("Scheduled run failed: down"),
            now,
        )
        .await;
    }
    let kinds = shape(&h).await;
    assert!(
        !kinds
            .iter()
            .any(|(_, subject, _)| *subject == first.to_string()),
        "the pruned run's result is gone: {kinds:?}"
    );
    assert_eq!(
        kinds,
        [(FeedKind::Problem, automation.id.to_string(), false)]
    );
}

#[tokio::test]
async fn the_scheduler_heartbeat_expires_old_items() {
    let (_d, h, _parent, child) = fixture().await;
    let automation = create(&h, child, "Digest").await;
    ran(
        &h,
        child,
        &automation,
        &failed("Scheduled run failed: down"),
        0,
    )
    .await;
    runs::heartbeat(&h.config.database, 1).expect("heartbeat");
    assert_eq!(items(&h).await.len(), 1);
    runs::heartbeat(&h.config.database, 31 * 24 * HOUR).expect("heartbeat");
    assert!(items(&h).await.is_empty());
}

#[tokio::test]
async fn the_owner_retries_a_problem_by_running_the_automation_now() {
    let (_d, h, _parent, child) = fixture().await;
    let automation = create(&h, child, "Digest").await;
    ran(
        &h,
        child,
        &automation,
        &failed("Scheduled run failed: down"),
        1,
    )
    .await;
    let owner = Uuid::new_v4();
    let control = HostAutomationControl::open(
        h.config.home.path(),
        h.host_id().await.expect("Host"),
        owner,
    )
    .expect("owner controls");
    let request = AutomationRunRequest {
        operation_id: Uuid::new_v4(),
    };
    let retried = control
        .run_now(owner, automation.id, request.clone(), 2)
        .await
        .expect("run now");
    assert_eq!(
        control
            .run_now(owner, automation.id, request.clone(), 3)
            .await
            .expect("an identical retry returns its receipt"),
        retried
    );
    assert!(matches!(
        control
            .run_now(
                owner,
                automation.id,
                AutomationRunRequest {
                    operation_id: Uuid::new_v4()
                },
                3
            )
            .await,
        Err(LocalHostError::Automation(AutomationError::Busy))
    ));
    assert!(matches!(
        control
            .run_now(Uuid::new_v4(), automation.id, request.clone(), 3)
            .await,
        Err(LocalHostError::Automation(AutomationError::Forbidden))
    ));
    let run = runs::next(&h.config.database, 4, false)
        .expect("pending")
        .expect("the owner's run is admitted");
    assert_eq!(run.id, request.operation_id);
    runs::finish(&h.config.database, run.id, &succeeded("Done."), 5).expect("finish");
    let found = shape(&h).await;
    assert_eq!(
        found[0],
        (FeedKind::Problem, automation.id.to_string(), true),
        "the retry's success closes the problem"
    );
}
