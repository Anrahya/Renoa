//! Replies that stop short, and what the trace says about each job.

use mnem::{
    dream,
    memory::{Entry, Memory},
    span::Span,
};
use serde_json::Value;
use uuid::Uuid;

use super::{FIRST_NIGHT, Fixture, NOON, dream_log, ms, receipt};
use crate::{
    RunStatus,
    plugins::host::memory::pass::{self, EVENTS},
};

/// The events of `run` named `name`, as logged.
fn events(run: Uuid, name: &str) -> Vec<(String, Value)> {
    EVENTS
        .lock()
        .expect("events")
        .iter()
        .filter(|(_, event, fields)| *event == name && fields["run_id"] == run.to_string())
        .map(|(level, _, fields)| ((*level).to_owned(), fields.clone()))
        .collect()
}

/// The summary saved for notes #0 and #1.
fn first_summary(memory: &Memory) -> Option<String> {
    let parent = Span::try_new(0, 4).expect("block");
    memory
        .zoom(parent)
        .expect("zoom")
        .into_iter()
        .find_map(|entry| match entry {
            Entry::Summary { block, text } if block == Span::try_new(0, 2).expect("block") => text,
            _ => None,
        })
}

#[tokio::test]
async fn a_cut_compaction_line_is_declined_and_a_cut_dream_is_rejected() {
    let fixture = Fixture::new().await;
    let first = fixture.notebook(Uuid::from_u128(1), 5);
    let second = fixture.notebook(Uuid::from_u128(2), 5);
    fixture.script(&[
        "cut:Walks the dog at da",
        "say:Walks the dog",
        "say:Walks the dog",
        "cut:<mind>\n## Durable\n- Walks the d",
    ]);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let (due, status) = fixture.night(ms(FIRST_NIGHT)).await;
    assert_eq!(status, RunStatus::Succeeded, "a cut reply is no failure");
    assert_eq!(
        fixture.receipts(due.run.id),
        [
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("dream", "rejected"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("dream", "dreamed"),
        ],
        "the second notebook goes on"
    );
    let saved = first_summary(&first).expect("a summary of #0 and #1");
    assert!(
        saved
            == "[by Fixture] Walked the dog at dawn, day 0; [by Fixture] Walked the dog at dawn, day 1",
        "a mechanical cut of the notes, never the half line: {saved}"
    );
    assert!(
        dream_log(&first).contains("rejected the draft"),
        "{}",
        dream_log(&first)
    );
    assert_eq!(dream::read_watermark(first.dir()).expect("watermark"), 0);
    assert_eq!(dream::read_watermark(second.dir()).expect("watermark"), 5);

    let jobs = events(due.run.id, "memory_job");
    let details: Vec<&str> = jobs
        .iter()
        .filter_map(|(_, fields)| fields["detail"].as_str())
        .collect();
    assert_eq!(details.first(), Some(&"CutInput"), "{details:?}");
    assert!(details.contains(&"ByModel"), "{details:?}");
    assert!(
        jobs.iter()
            .any(|(_, fields)| fields["outcome"] == "rejected"
                && fields["detail"].as_str().is_some_and(|why| !why.is_empty())),
        "the rejection says why: {jobs:?}"
    );
    let cut = events(due.run.id, "memory_turn_cut");
    assert_eq!(cut.len(), 2, "{cut:?}");
    assert!(cut.iter().all(|(level, _)| level == "warn"));
}

#[tokio::test]
async fn a_cut_reply_with_no_text_applies_nothing_and_stops_only_its_notebook() {
    let fixture = Fixture::new().await;
    let first = fixture.notebook(Uuid::from_u128(1), 5);
    let second = fixture.notebook(Uuid::from_u128(2), 5);
    fixture.script(&["cut:", "cut:"]);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let (due, status) = fixture.night(ms(FIRST_NIGHT)).await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(
        fixture.receipts(due.run.id),
        [
            receipt("compaction", "truncated"),
            receipt("dream", "truncated"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("dream", "dreamed"),
        ]
    );
    assert_eq!(dream::read_watermark(second.dir()).expect("watermark"), 5);
    for night in ["2026-11-04T05:30:00Z", "2026-11-05T05:30:00Z"] {
        fixture.script(&["cut:", "cut:"]);
        let (due, _) = fixture.night(ms(night)).await;
        assert_eq!(
            fixture.receipts(due.run.id),
            [
                receipt("compaction", "truncated"),
                receipt("dream", "truncated")
            ]
        );
    }
    assert_eq!(
        first
            .compaction_job()
            .expect("job")
            .map(|job| job.id.attempt),
        Some(0),
        "no summary was saved and no try kept"
    );
    assert_eq!(dream::read_watermark(first.dir()).expect("watermark"), 0);
    assert_eq!(
        dream_log(&first),
        "",
        "nothing was counted toward giving up"
    );
}

#[tokio::test]
async fn compaction_stops_at_its_bound_and_the_rest_waits_for_the_next_night() {
    let fixture = Fixture::new().await;
    let memory = fixture.notebook(Uuid::new_v4(), 40);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let (due, status) = fixture.night(ms(FIRST_NIGHT)).await;
    assert_eq!(status, RunStatus::Succeeded);
    let kinds = fixture.kinds();
    assert_eq!(
        kinds.iter().filter(|kind| *kind == "compaction").count(),
        pass::COMPACTION_TURNS
    );
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("dream"),
        "Dream still runs"
    );
    assert!(
        fixture
            .trigger()
            .await
            .last_run
            .and_then(|run| run.summary)
            .expect("summary")
            .ends_with("Compaction reached its bound on 1 notebook with more due."),
    );
    let bounded = events(due.run.id, "memory_compaction_bounded");
    assert_eq!(bounded.len(), 1, "{bounded:?}");
    assert_eq!(bounded[0].1["turns"], pass::COMPACTION_TURNS);
    assert!(memory.compaction_job().expect("job").is_some());

    fixture.night(ms("2026-11-04T05:30:00Z")).await;
    assert!(
        fixture.kinds().len() > kinds.len(),
        "the next night goes on"
    );
}
