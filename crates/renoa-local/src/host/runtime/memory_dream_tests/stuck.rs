//! A memory job, or a memory session, that cannot go on: bounded by runs, and
//! never wedging the nights after it.

use std::fs;

use mnem::dream;
use uuid::Uuid;

use super::{FIRST_NIGHT, Fixture, NOON, dream_log, ms, receipt};
use crate::{
    RunStatus, ScheduledRun,
    plugins::host::memory::{
        jobs::JobKind,
        pass::fault::{self, Point},
        sessions,
    },
};

const NIGHTS: [&str; 4] = [
    "2026-11-03T22:30:00Z",
    "2026-11-04T22:30:00Z",
    "2026-11-05T22:30:00Z",
    "2026-11-06T22:30:00Z",
];

/// The current memory session generation of `agent`, and why it moved.
fn generation(fixture: &Fixture) -> (u32, Option<String>) {
    let connection = rusqlite::Connection::open(fixture.database()).expect("catalog");
    let current = sessions::current(&connection, fixture.main).expect("generation");
    let reason = sessions::reason(&connection, fixture.main, current).expect("reason");
    (current, reason)
}

fn session_stored(fixture: &Fixture, generation: u32) -> bool {
    fixture
        .host
        .has_session(sessions::session_id(fixture.main, generation))
        .expect("sessions")
}

async fn summary(fixture: &Fixture) -> String {
    fixture
        .trigger()
        .await
        .last_run
        .and_then(|run| run.summary)
        .expect("summary")
}

fn breaking(fixture: &Fixture) -> impl FnOnce() + Send + 'static {
    let marker = fixture.root.join("broken");
    move || fs::write(marker, "").expect("break the model")
}

fn mend(fixture: &Fixture) {
    fs::remove_file(fixture.root.join("broken")).expect("the model is back");
}

/// The first night, with the model gone once a job of `kind` is frozen.
async fn first_night_breaks_at(fixture: &Fixture, kind: JobKind) -> uuid::Uuid {
    assert!(fixture.next(ms(NOON)).await.is_none());
    let due = fixture.next(ms(FIRST_NIGHT)).await.expect("due");
    fault::on(due.run.id, Point::Frozen(kind), breaking(fixture));
    assert_eq!(
        fixture.finish(&due, ms(FIRST_NIGHT)).await,
        RunStatus::Failed
    );
    assert!(summary(fixture).await.contains("(1 of 3 runs)"));
    due.run.id
}

#[tokio::test]
async fn a_job_that_cannot_run_for_three_nights_is_abandoned_and_the_fourth_goes_on() {
    let fixture = Fixture::new().await;
    let memory = fixture.notebook(Uuid::new_v4(), 5);
    let first = first_night_breaks_at(&fixture, JobKind::Compaction).await;
    assert_eq!(fixture.night(ms(NIGHTS[0])).await.1, RunStatus::Failed);
    assert!(summary(&fixture).await.contains("(2 of 3 runs)"));
    assert_eq!(fixture.receipts(first), [("compaction".to_owned(), None)]);
    assert_eq!(fixture.night(ms(NIGHTS[1])).await.1, RunStatus::Failed);
    assert!(summary(&fixture).await.contains("could not run on 3 runs"));
    assert_eq!(
        fixture.receipts(first),
        [receipt("compaction", "abandoned")],
        "abandoned on the third night"
    );
    let (moved, reason) = generation(&fixture);
    assert_eq!(moved, 1);
    assert!(
        reason
            .as_deref()
            .is_some_and(|why| why.contains("could not run on 3 runs")),
        "{reason:?}"
    );
    assert!(
        fixture.calls().is_empty(),
        "the abandoned job was never answered"
    );

    mend(&fixture);
    let (due, status) = fixture.night(ms(NIGHTS[2])).await;
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(
        fixture.kinds(),
        ["compaction", "compaction", "compaction", "dream"]
    );
    assert_eq!(
        fixture.receipts(due.run.id),
        [
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("dream", "dreamed"),
        ]
    );
    assert_eq!(dream::read_watermark(memory.dir()).expect("watermark"), 5);
    assert!(
        session_stored(&fixture, 0),
        "the old session stays readable"
    );
    assert!(
        session_stored(&fixture, 1),
        "the fourth night ran in the new one"
    );
}

#[tokio::test]
async fn an_abandoned_dream_is_never_applied_or_counted() {
    let fixture = Fixture::new().await;
    let memory = fixture.notebook(Uuid::new_v4(), 5);
    let first = first_night_breaks_at(&fixture, JobKind::Dream).await;
    fixture.night(ms(NIGHTS[0])).await;
    fixture.night(ms(NIGHTS[1])).await;
    assert_eq!(
        fixture.receipts(first).last(),
        Some(&receipt("dream", "abandoned"))
    );
    assert_eq!(dream_log(&memory), "", "Mnem never saw it");
    assert_eq!(dream::read_watermark(memory.dir()).expect("watermark"), 0);

    mend(&fixture);
    let (due, status) = fixture.night(ms(NIGHTS[2])).await;
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(fixture.receipts(due.run.id), [receipt("dream", "dreamed")]);
    assert_eq!(dream::read_watermark(memory.dir()).expect("watermark"), 5);
}

#[tokio::test]
async fn a_job_whose_session_cannot_open_on_three_runs_is_abandoned() {
    let fixture = Fixture::new().await;
    fixture.notebook(Uuid::new_v4(), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let due = fixture.next(ms(FIRST_NIGHT)).await.expect("due");
    fault::arm(due.run.id, Point::Frozen(JobKind::Compaction));
    assert!(
        fixture
            .scheduler
            .run_for_plugin(&fixture.host, &due, ms(FIRST_NIGHT))
            .await
            .is_err()
    );
    fs::write(fixture.root.join("broken"), "").expect("break the model");
    // Every try from here fails to open the session, never to run the job.
    let resumed = fixture
        .next(ms(FIRST_NIGHT) + 60_000)
        .await
        .expect("resumed");
    assert_eq!(
        fixture.finish(&resumed, ms(FIRST_NIGHT) + 60_000).await,
        RunStatus::Failed
    );
    let opened = summary(&fixture).await;
    assert!(
        opened.contains("could not open") && opened.contains("(1 of 3 runs)"),
        "{opened}"
    );
    fixture.night(ms(NIGHTS[0])).await;
    fixture.night(ms(NIGHTS[1])).await;
    assert_eq!(
        fixture.receipts(due.run.id),
        [receipt("compaction", "abandoned")]
    );
    assert_eq!(generation(&fixture).0, 1);
    mend(&fixture);
    assert_eq!(fixture.night(ms(NIGHTS[2])).await.1, RunStatus::Succeeded);
}

/// Three runs fail the first job; the third is counted and then crashes
/// before the agent moves on.
async fn counted_three_times_then_crashed() -> (Fixture, Uuid, ScheduledRun) {
    let fixture = Fixture::new().await;
    fixture.notebook(Uuid::new_v4(), 5);
    let first = first_night_breaks_at(&fixture, JobKind::Compaction).await;
    fixture.night(ms(NIGHTS[0])).await;
    let third = fixture.next(ms(NIGHTS[1])).await.expect("due");
    fault::arm(third.run.id, Point::Counted);
    assert!(
        fixture
            .scheduler
            .run_for_plugin(&fixture.host, &third, ms(NIGHTS[1]))
            .await
            .is_err()
    );
    assert_eq!(fixture.receipts(first), [("compaction".to_owned(), None)]);
    assert_eq!(generation(&fixture).0, 0, "not moved on yet");
    (fixture, first, third)
}

#[tokio::test]
async fn a_crash_after_the_third_count_abandons_on_the_next_failure() {
    let (fixture, first, third) = counted_three_times_then_crashed().await;
    let resumed = fixture.next(ms(NIGHTS[1]) + 60_000).await.expect("resumed");
    assert_eq!(resumed.run.id, third.run.id);
    assert_eq!(
        fixture.finish(&resumed, ms(NIGHTS[1]) + 60_000).await,
        RunStatus::Failed
    );
    assert!(
        summary(&fixture).await.contains("could not run on 3 runs"),
        "the resumed run is not counted again, and abandons"
    );
    assert_eq!(
        fixture.receipts(first),
        [receipt("compaction", "abandoned")]
    );
    assert_eq!(generation(&fixture).0, 1);
}

#[tokio::test]
async fn a_crash_after_the_third_count_settles_on_the_next_success() {
    let (fixture, first, third) = counted_three_times_then_crashed().await;
    mend(&fixture);
    let resumed = fixture.next(ms(NIGHTS[1]) + 60_000).await.expect("resumed");
    assert_eq!(resumed.run.id, third.run.id);
    assert_eq!(
        fixture.finish(&resumed, ms(NIGHTS[1]) + 60_000).await,
        RunStatus::Succeeded
    );
    assert!(
        !fixture
            .receipts(first)
            .contains(&receipt("compaction", "abandoned")),
        "the counted job was not abandoned"
    );
    assert_eq!(
        fixture.receipts(resumed.run.id).last(),
        Some(&receipt("dream", "dreamed")),
        "the resumed run went on to dream"
    );
    assert_eq!(generation(&fixture).0, 0);
}

#[tokio::test]
async fn a_stored_session_that_cannot_open_on_three_runs_moves_the_agent_on() {
    let fixture = Fixture::new().await;
    let memory = fixture.notebook(Uuid::new_v4(), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());
    assert_eq!(fixture.night(ms(FIRST_NIGHT)).await.1, RunStatus::Succeeded);
    fs::write(fixture.root.join("broken"), "").expect("break the model");
    super::add_notes(&memory, 4);
    for (tries, night) in [(1, NIGHTS[0]), (2, NIGHTS[1])] {
        assert_eq!(fixture.night(ms(night)).await.1, RunStatus::Failed);
        assert!(
            summary(&fixture)
                .await
                .contains(&format!("({tries} of 3 runs)")),
            "{}",
            summary(&fixture).await
        );
        assert_eq!(generation(&fixture).0, 0);
    }
    assert_eq!(fixture.night(ms(NIGHTS[2])).await.1, RunStatus::Failed);
    let (moved, reason) = generation(&fixture);
    assert_eq!(moved, 1, "nothing in it was unfinished");
    assert!(reason.is_some_and(|why| why.contains("could not open")));

    // A session that was never stored is not moved past: a new one would
    // fail the same way.
    assert_eq!(fixture.night(ms(NIGHTS[3])).await.1, RunStatus::Failed);
    assert_eq!(generation(&fixture).0, 1);
    let unstored = summary(&fixture).await;
    assert!(!unstored.contains("of 3 runs"), "never counted: {unstored}");

    mend(&fixture);
    assert_eq!(
        fixture.night(ms("2026-11-07T22:30:00Z")).await.1,
        RunStatus::Succeeded
    );
    assert!(session_stored(&fixture, 1));
    assert_eq!(dream::read_watermark(memory.dir()).expect("watermark"), 9);

    // A stored session after the first is counted on its own generation.
    super::add_notes(&memory, 4);
    fs::write(fixture.root.join("broken"), "").expect("break the model");
    assert_eq!(
        fixture.night(ms("2026-11-08T22:30:00Z")).await.1,
        RunStatus::Failed
    );
    let later = summary(&fixture).await;
    assert!(later.contains("(1 of 3 runs)"), "{later}");
}

/// The run due at `at`, counted once and then crashed, and resumed.
async fn counted_then_resumed(fixture: &Fixture, at: &str) -> String {
    let due = fixture.next(ms(at)).await.expect("due");
    fault::arm(due.run.id, Point::Counted);
    assert!(
        fixture
            .scheduler
            .run_for_plugin(&fixture.host, &due, ms(at))
            .await
            .is_err()
    );
    let resumed = fixture.next(ms(at) + 60_000).await.expect("resumed");
    assert_eq!(resumed.run.id, due.run.id);
    assert_eq!(
        fixture.finish(&resumed, ms(at) + 60_000).await,
        RunStatus::Failed
    );
    summary(fixture).await
}

#[tokio::test]
async fn a_job_counts_one_run_once_however_often_it_tried() {
    let fixture = Fixture::new().await;
    let memory = fixture.notebook(Uuid::new_v4(), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());
    assert_eq!(fixture.night(ms(FIRST_NIGHT)).await.1, RunStatus::Succeeded);
    super::add_notes(&memory, 4);
    let due = fixture.next(ms(NIGHTS[0])).await.expect("due");
    fault::on(
        due.run.id,
        Point::Frozen(JobKind::Compaction),
        breaking(&fixture),
    );
    fault::arm(due.run.id, Point::Counted);
    assert!(
        fixture
            .scheduler
            .run_for_plugin(&fixture.host, &due, ms(NIGHTS[0]))
            .await
            .is_err()
    );
    let resumed = fixture.next(ms(NIGHTS[0]) + 60_000).await.expect("resumed");
    assert_eq!(resumed.run.id, due.run.id);
    assert_eq!(
        fixture.finish(&resumed, ms(NIGHTS[0]) + 60_000).await,
        RunStatus::Failed
    );
    let once = summary(&fixture).await;
    assert!(once.contains("(1 of 3 runs)"), "{once}");
    assert_eq!(fixture.night(ms(NIGHTS[1])).await.1, RunStatus::Failed);
    assert!(summary(&fixture).await.contains("(2 of 3 runs)"));
}

#[tokio::test]
async fn a_session_counts_one_run_once_and_forgets_its_runs_once_it_opens() {
    let fixture = Fixture::new().await;
    let memory = fixture.notebook(Uuid::new_v4(), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());
    assert_eq!(fixture.night(ms(FIRST_NIGHT)).await.1, RunStatus::Succeeded);
    super::add_notes(&memory, 4);
    fs::write(fixture.root.join("broken"), "").expect("break the model");
    let once = counted_then_resumed(&fixture, NIGHTS[0]).await;
    assert!(once.contains("(1 of 3 runs)"), "{once}");
    assert_eq!(fixture.night(ms(NIGHTS[1])).await.1, RunStatus::Failed);
    assert!(summary(&fixture).await.contains("(2 of 3 runs)"));

    mend(&fixture);
    assert_eq!(fixture.night(ms(NIGHTS[2])).await.1, RunStatus::Succeeded);
    super::add_notes(&memory, 4);
    fs::write(fixture.root.join("broken"), "").expect("break the model");
    assert_eq!(fixture.night(ms(NIGHTS[3])).await.1, RunStatus::Failed);
    let again = summary(&fixture).await;
    assert!(again.contains("(1 of 3 runs)"), "counted afresh: {again}");
    assert_eq!(generation(&fixture).0, 0);
}

#[tokio::test]
async fn sessions_that_cannot_be_listed_never_move_the_agent_on() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = Fixture::new().await;
    fixture.notebook(Uuid::new_v4(), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());
    assert_eq!(fixture.night(ms(FIRST_NIGHT)).await.1, RunStatus::Succeeded);
    let sessions = fixture.data().join("sessions");
    let mode = fs::metadata(&sessions).expect("sessions").permissions();
    let lock = |locked: bool| {
        let mode = if locked {
            fs::Permissions::from_mode(0o000)
        } else {
            mode.clone()
        };
        fs::set_permissions(&sessions, mode).expect("sessions mode");
    };
    lock(true);
    if fs::read_dir(&sessions).is_ok() {
        // A superuser reads through the mode, so nothing can be shown here.
        lock(false);
        return;
    }
    lock(false);
    for night in &NIGHTS[..3] {
        let due = fixture.next(ms(night)).await.expect("due");
        lock(true);
        let status = fixture.finish(&due, ms(night)).await;
        lock(false);
        assert_eq!(status, RunStatus::Failed);
        let stopped = summary(&fixture).await;
        assert!(stopped.contains("could not be read"), "{stopped}");
    }
    assert_eq!(
        generation(&fixture).0,
        0,
        "an unreadable listing is no reason to move"
    );
}

#[tokio::test]
async fn moving_on_from_a_generation_the_agent_left_changes_nothing() {
    let fixture = Fixture::new().await;
    fixture.notebook(Uuid::new_v4(), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let due = fixture.next(ms(FIRST_NIGHT)).await.expect("due");
    fault::arm(due.run.id, Point::Frozen(JobKind::Compaction));
    assert!(
        fixture
            .scheduler
            .run_for_plugin(&fixture.host, &due, ms(FIRST_NIGHT))
            .await
            .is_err()
    );
    let command: String = rusqlite::Connection::open(fixture.database())
        .expect("catalog")
        .query_row(
            "SELECT command_id FROM host_memory_jobs WHERE receipt IS NULL",
            [],
            |row| row.get(0),
        )
        .expect("the frozen job");
    let command = Uuid::parse_str(&command).expect("command id");
    let refused = sessions::move_on(
        fixture.database(),
        fixture.main,
        1,
        Some(command),
        "a stale observer",
        ms(FIRST_NIGHT),
    );
    assert!(refused.is_err());
    assert_eq!(
        fixture.receipts(due.run.id),
        [("compaction".to_owned(), None)],
        "the abandon rolled back with the refused move"
    );
    assert_eq!(generation(&fixture).0, 0);
}
