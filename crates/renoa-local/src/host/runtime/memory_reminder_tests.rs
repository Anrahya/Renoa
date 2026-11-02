//! `renoa.memory`'s reminders through the real scheduler, conversation
//! sessions and catalog, with Mnem's own notebooks and an injected clock. The
//! node's side, submitting a run's text to its conversation, is the
//! boundary: a test records the run's result as the node does.

use std::path::PathBuf;

use jiff::civil::DateTime;
use mnem::{
    heartbeat::NudgeInput,
    memory::{Memory, NudgeRun},
};
use uuid::Uuid;

use super::memory_pass_tests::{ENOUGH, Fixture, MINUTE, NOON, ms};
use crate::{RunOutcome, RunStatus, ScheduledRun, plugins::host::memory::fixtures};

/// The reminder's due time, five minutes after [`NOON`], in the notebook
/// zone, and as an instant.
const DUE: &str = "2026-11-02T12:05";
const DUE_AT: &str = "2026-11-02T06:35:00Z";
const DAY: i64 = 24 * 60 * MINUTE;
const NOTE_TOOL: &str = "memory_note";

fn at(raw: &str) -> DateTime {
    raw.parse().expect("a wall time")
}

fn data(fixture: &Fixture) -> PathBuf {
    fixture.root.join("data")
}

fn notebook_of(fixture: &Fixture, person: Uuid) -> PathBuf {
    fixtures::notebook(&data(fixture), person)
}

fn notebook(fixture: &Fixture, person: Uuid) -> Memory {
    Memory::create(&notebook_of(fixture, person)).expect("a notebook")
}

/// A note of `person`'s that sets a reminder at `due` for `what`.
fn remind(fixture: &Fixture, person: Uuid, due: &str, what: &str) -> Memory {
    let memory = notebook(fixture, person);
    memory
        .note(
            "Arcee",
            &format!("[at {due}] {what}"),
            at("2026-11-02T11:45"),
        )
        .expect("a reminder note");
    memory
}

/// The run due at `now` that the scheduler sends to an agent or skips, while
/// a plugin's run is in flight beside it, so a plugin's own timers stay out.
async fn next(fixture: &Fixture, now: i64) -> Option<ScheduledRun> {
    fixture
        .scheduler
        .next_run(now, true)
        .await
        .expect("next run")
}

/// Records `due`'s run as the node does once the agent answered.
async fn answer(fixture: &Fixture, due: &ScheduledRun, now: i64) {
    fixture
        .scheduler
        .finish_run(
            due.run.id,
            RunOutcome::Succeeded {
                answer: "I called the bank; the card is blocked.".to_owned(),
                failed_tool_calls: 0,
                landing: crate::RunLanding::Conversation,
            },
            now,
        )
        .await
        .expect("record the run");
}

fn mnem_message(memory: &Memory, item: &str, due: &str, now: &str) -> NudgeRun {
    memory
        .nudge_run(&NudgeInput {
            id: item,
            due: at(due),
            now: at(now),
            note_tool: NOTE_TOOL,
        })
        .expect("the agenda reads")
        .expect("the item is due")
}

fn timed(memory: &Memory, now: &str) -> Vec<String> {
    memory
        .agenda(at(now))
        .expect("the agenda reads")
        .timed
        .into_iter()
        .map(|item| item.id)
        .collect()
}

fn fired(fixture: &Fixture, person: Uuid) -> usize {
    mnem::fired::read(&notebook_of(fixture, person))
        .expect("the fired ledger")
        .len()
}

fn catalog(fixture: &Fixture) -> rusqlite::Connection {
    rusqlite::Connection::open(fixture.database()).expect("catalog")
}

/// Each reminder's item and whether it was recorded fired, by item.
fn reminders(fixture: &Fixture) -> Vec<(String, bool)> {
    catalog(fixture)
        .prepare("SELECT item_id, fired FROM host_memory_reminders ORDER BY item_id, due")
        .expect("reminders")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("reminders")
        .collect::<Result<_, _>>()
        .expect("reminders")
}

/// The reminder triggers inspection lists, with whether each is enabled.
async fn listed(fixture: &Fixture) -> Vec<(Uuid, bool)> {
    fixture
        .snapshot()
        .await
        .automations
        .into_iter()
        .filter(|automation| automation.name == "renoa.memory reminder")
        .map(|automation| (automation.id, automation.enabled))
        .collect()
}

/// Whether the catalog still holds `automation` in any form.
fn stored(fixture: &Fixture, automation: Uuid) -> bool {
    catalog(fixture)
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM host_automations WHERE id = ?1)
                 OR EXISTS(SELECT 1 FROM host_plugin_triggers WHERE automation_id = ?1)
                 OR EXISTS(SELECT 1 FROM host_memory_reminders WHERE automation_id = ?1)",
            [automation.to_string()],
            |row| row.get(0),
        )
        .expect("read")
}

/// Deletes `automation`'s conversation as the node's cleanup does, once the
/// Host marked it, then converges again at `now`.
async fn conversation_gone(fixture: &Fixture, automation: Uuid, now: i64) {
    assert!(
        fixture
            .scheduler
            .conversations_to_delete()
            .await
            .expect("marks")
            .contains(&automation),
        "its conversation is marked for deletion"
    );
    fixture
        .scheduler
        .conversation_deleted(automation)
        .await
        .expect("unmark");
    assert!(next(fixture, now).await.is_none());
}

#[tokio::test]
async fn a_due_reminder_goes_to_the_main_agent_in_mnem_words_in_its_latest_conversation() {
    let fixture = Fixture::new().await;
    let noon = ms(NOON);
    let (older, latest, person) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    fixture.say(older, person, noon - 30 * MINUTE, ENOUGH).await;
    fixture
        .say(latest, person, noon - 20 * MINUTE, ENOUGH)
        .await;
    let memory = remind(&fixture, person, DUE, "Call the bank about the card");

    assert!(
        next(&fixture, noon).await.is_none(),
        "not due before its time"
    );
    let due = next(&fixture, ms(DUE_AT))
        .await
        .expect("the reminder is due");
    assert_eq!(due.plugin, None, "the agent's to run, not the plugin's");
    assert_eq!(due.run.agent_id, fixture.main);
    assert_eq!(due.origin_session_id, Some(latest));
    let run = mnem_message(&memory, "r0", DUE, DUE);
    assert!(run.notify);
    assert_eq!(due.run.submission, run.message, "Mnem's message, verbatim");
    assert!(
        crate::host::automations::delivering(fixture.database(), due.run.id).expect("read"),
        "a surface's claim waits for it as for an agent's own run"
    );
}

#[tokio::test]
async fn a_finished_reminder_is_recorded_fired_once_and_never_runs_again() {
    let fixture = Fixture::new().await;
    let noon = ms(NOON);
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    fixture
        .say(session, person, noon - 20 * MINUTE, ENOUGH)
        .await;
    let memory = remind(&fixture, person, DUE, "Call the bank about the card");
    let due = next(&fixture, ms(DUE_AT)).await.expect("due");
    let after = ms(DUE_AT) + MINUTE;
    answer(&fixture, &due, after).await;
    assert_eq!(
        timed(&memory, "2026-11-02T12:06"),
        ["r0"],
        "listed until the Host records it"
    );

    assert!(next(&fixture, after + MINUTE).await.is_none());
    assert!(
        timed(&memory, "2026-11-02T12:07").is_empty(),
        "recorded fired once its run is recorded"
    );
    assert_eq!(fired(&fixture, person), 1);
    assert_eq!(reminders(&fixture), [("r0".to_owned(), true)]);

    // A crash after Mnem recorded it, before the Host marked it.
    catalog(&fixture)
        .execute("UPDATE host_memory_reminders SET fired = 0", [])
        .expect("unmark");
    assert!(next(&fixture, after + 2 * MINUTE).await.is_none());
    assert_eq!(fired(&fixture, person), 1, "recorded once");
    assert_eq!(reminders(&fixture), [("r0".to_owned(), true)]);

    let observed = fixture
        .snapshot()
        .await
        .automations
        .into_iter()
        .find(|automation| automation.id == due.run.automation_id)
        .expect("inspection lists the reminder");
    assert_eq!(observed.plugin.as_deref(), Some("renoa.memory"));
    let last = observed.last_run.expect("its run");
    assert_eq!(last.status, RunStatus::Succeeded);
    assert_eq!(
        last.summary, None,
        "the agent's reply stays out of inspection"
    );

    assert!(
        next(&fixture, after + 2 * DAY).await.is_none(),
        "it never runs again"
    );
}

#[tokio::test]
async fn a_quiet_item_runs_in_a_conversation_of_its_own() {
    let fixture = Fixture::new().await;
    let noon = ms(NOON);
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    fixture
        .say(session, person, noon - 20 * MINUTE, ENOUGH)
        .await;
    fixtures::dream_with_nudges(
        &notebook_of(&fixture, person),
        "Writes letters by hand",
        |last| {
            format!(
                "- letter | {DUE} | - | Draft the letter to the bank | Seen again (#{last}) | quiet\n"
            )
        },
    );

    let due = next(&fixture, ms(DUE_AT)).await.expect("due");
    let run = mnem_message(&notebook(&fixture, person), "letter", DUE, DUE);
    assert!(!run.notify);
    assert_eq!(due.run.submission, run.message);
    assert_eq!(
        due.origin_session_id, None,
        "not in the person's conversation, where they would see the reply"
    );
    assert_eq!(due.plugin, None);
}

#[tokio::test]
async fn a_reminder_no_longer_due_when_admitted_is_skipped_and_not_run_again() {
    let fixture = Fixture::new().await;
    let noon = ms(NOON);
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    fixture
        .say(session, person, noon - 20 * MINUTE, ENOUGH)
        .await;
    remind(&fixture, person, DUE, "Call the bank about the card");
    assert!(
        next(&fixture, noon).await.is_none(),
        "converged before it is due"
    );

    // The Host was down past the item's grace: it is stale when admitted.
    let late = ms(DUE_AT) + 8 * DAY;
    let skipped = next(&fixture, late).await.expect("admitted");
    let result = skipped.run.result.expect("finished when admitted");
    assert_eq!(result.status, RunStatus::Skipped);
    assert!(result.output.contains("No longer due"), "{}", result.output);
    assert!(
        next(&fixture, late + MINUTE).await.is_none(),
        "not admitted again"
    );
    assert_eq!(fired(&fixture, person), 0, "a skipped run fired nothing");
}

#[tokio::test]
async fn a_moved_reminder_retires_its_trigger_which_goes_once_its_conversation_is_gone() {
    let fixture = Fixture::new().await;
    let noon = ms(NOON);
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    fixture
        .say(session, person, noon - 20 * MINUTE, ENOUGH)
        .await;
    let memory = remind(&fixture, person, DUE, "Call the bank about the card");
    assert!(next(&fixture, noon).await.is_none());
    let [(old, true)] = listed(&fixture).await[..] else {
        panic!("one armed reminder");
    };

    memory
        .note(
            "Arcee",
            "[updates #0] [at 2026-11-02T13:00] Call the bank about the card after lunch",
            at("2026-11-02T11:50"),
        )
        .expect("a correcting note");
    assert!(next(&fixture, noon + MINUTE).await.is_none());
    let listed_now = listed(&fixture).await;
    assert!(
        !listed_now.iter().any(|(id, _)| *id == old),
        "the old trigger is deleted"
    );
    assert_eq!(listed_now.len(), 1, "one trigger for the new time");
    assert!(stored(&fixture, old), "kept until its conversation is gone");

    conversation_gone(&fixture, old, noon + 2 * MINUTE).await;
    assert!(!stored(&fixture, old), "removed outright");
    assert_eq!(reminders(&fixture), [("r1".to_owned(), false)]);

    let due = next(&fixture, ms("2026-11-02T07:30:00Z"))
        .await
        .expect("the new time is due");
    assert_eq!(
        due.run.submission,
        mnem_message(&memory, "r1", "2026-11-02T13:00", "2026-11-02T13:00").message
    );
}

#[tokio::test]
async fn a_reminder_that_ran_stays_while_its_run_is_kept_and_goes_after() {
    let fixture = Fixture::new().await;
    let noon = ms(NOON);
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    fixture
        .say(session, person, noon - 20 * MINUTE, ENOUGH)
        .await;
    remind(&fixture, person, DUE, "Call the bank about the card");
    let due = next(&fixture, ms(DUE_AT)).await.expect("due");
    let automation = due.run.automation_id;
    answer(&fixture, &due, ms(DUE_AT) + MINUTE).await;
    assert!(next(&fixture, ms(DUE_AT) + 2 * MINUTE).await.is_none());
    assert_eq!(
        listed(&fixture).await,
        [(automation, false)],
        "kept, disabled, with its run"
    );

    let later = ms(DUE_AT) + 31 * DAY;
    fixture
        .scheduler
        .heartbeat(later)
        .await
        .expect("expire runs");
    assert!(next(&fixture, later).await.is_none());
    assert!(
        listed(&fixture).await.is_empty(),
        "deleted once its run went"
    );
    conversation_gone(&fixture, automation, later + MINUTE).await;
    assert!(!stored(&fixture, automation));
}

#[tokio::test]
async fn a_notebook_that_cannot_be_read_keeps_its_reminders_and_holds_up_no_one_else() {
    let fixture = Fixture::new().await;
    let noon = ms(NOON);
    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
    fixture
        .say(Uuid::new_v4(), first, noon - 30 * MINUTE, ENOUGH)
        .await;
    fixture
        .say(Uuid::new_v4(), second, noon - 20 * MINUTE, ENOUGH)
        .await;
    let memory = remind(&fixture, first, DUE, "Call the bank about the card");
    remind(&fixture, second, "2026-11-02T12:10", "Water the plants");
    assert!(next(&fixture, noon).await.is_none());
    assert_eq!(listed(&fixture).await.len(), 2);

    let broken = notebook_of(&fixture, second);
    std::fs::rename(&broken, broken.with_extension("moved")).expect("move the notebook");
    std::fs::write(&broken, "not a notebook").expect("a file in its place");
    memory
        .note(
            "Arcee",
            "[at 2026-11-02T12:30] Pick up the parcel",
            at("2026-11-02T11:55"),
        )
        .expect("another reminder");

    let due = next(&fixture, ms(DUE_AT)).await.expect("the first one's");
    answer(&fixture, &due, ms(DUE_AT) + MINUTE).await;
    assert!(next(&fixture, ms(DUE_AT) + 2 * MINUTE).await.is_none());
    let armed = listed(&fixture)
        .await
        .into_iter()
        .filter(|(_, enabled)| *enabled)
        .count();
    assert_eq!(armed, 2, "the new reminder, and the unreadable one's");
    assert!(
        crate::plugins::host::memory::pass::EVENTS
            .lock()
            .expect("events")
            .iter()
            .any(|(_, name, fields)| {
                *name == "memory_reminders_unread"
                    && fields["principal_id"] == serde_json::json!(second)
            }),
        "logged"
    );
}

#[tokio::test]
async fn schema_47_adds_reminders_to_a_schema_46_catalog() {
    let fixture = Fixture::new().await;
    let database = fixture.database().to_path_buf();
    let noon = ms(NOON);
    assert!(next(&fixture, noon).await.is_none());
    let connection = rusqlite::Connection::open(&database).expect("catalog");
    crate::host::catalog::restore_schema_46(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 46 WHERE singleton = 1;
             PRAGMA user_version = 46;",
        )
        .expect("store schema 46");
    drop(connection);
    crate::host::catalog::initialize(&database).expect("upgrade schema 46");
    crate::host::catalog::initialize(&database).expect("reopen the upgraded catalog");

    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    fixture
        .say(session, person, noon - 20 * MINUTE, ENOUGH)
        .await;
    remind(&fixture, person, DUE, "Call the bank about the card");
    let due = next(&fixture, ms(DUE_AT))
        .await
        .expect("the reminder table is in place");
    assert_eq!(due.origin_session_id, Some(session));
}
