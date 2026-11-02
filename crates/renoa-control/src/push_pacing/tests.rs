use std::time::Duration;

use tokio::time::Instant;

use super::{MAX_RETRY_DELAY, Pacing, Ready};
use crate::{DeviceId, PushToken, fcm::WakeOutcome, push_tokens::PushTarget};

const GAP: Duration = Duration::from_secs(10);
const GAVE_UP: WakeOutcome = WakeOutcome::GaveUp("UNAVAILABLE");

fn phone() -> PushTarget {
    PushTarget {
        device_id: DeviceId::new(),
        token: PushToken::new("fcm:first"),
    }
}

#[test]
fn a_new_token_drops_the_backoff_its_old_one_earned_but_not_the_gap() {
    let old = phone();
    let new = PushTarget {
        token: PushToken::new("fcm:second"),
        ..old.clone()
    };
    let now = Instant::now();
    let backed_off = || {
        let pacing = Pacing::default();
        for _ in 0..5 {
            pacing.ended(&old, GAVE_UP, now);
        }
        assert_eq!(
            pacing.ready(&old, Duration::ZERO, now),
            Ready::At(now + Duration::from_secs(32))
        );
        pacing
    };

    let pacing = backed_off();
    assert_eq!(
        pacing.ready(&new, GAP, now),
        Ready::At(now + GAP),
        "the gap still holds"
    );
    assert_eq!(pacing.ready(&new, Duration::ZERO, now), Ready::Now);

    assert_eq!(
        backed_off().ended(&new, GAVE_UP, now),
        Some(Duration::from_secs(2)),
        "the new token's run of failures starts afresh"
    );
}

#[test]
fn a_device_that_keeps_giving_up_waits_twice_as_long_each_time_up_to_ten_minutes() {
    let pacing = Pacing::default();
    let device = phone();
    let mut now = Instant::now();
    let mut delays = Vec::new();
    for _ in 0..11 {
        pacing.started(device.device_id);
        let delay = pacing
            .ended(&device, GAVE_UP, now)
            .expect("a send that gave up waits");
        assert_eq!(
            pacing.ready(&device, Duration::ZERO, now),
            Ready::At(now + delay)
        );
        delays.push(delay.as_secs());
        now += delay;
        assert_eq!(pacing.ready(&device, Duration::ZERO, now), Ready::Now);
    }
    assert_eq!(delays, [2, 4, 8, 16, 32, 64, 128, 256, 512, 600, 600]);
    assert_eq!(MAX_RETRY_DELAY, Duration::from_mins(10));
}

#[test]
fn a_delivered_send_resets_the_backoff() {
    let pacing = Pacing::default();
    let device = phone();
    let now = Instant::now();
    for _ in 0..3 {
        pacing.ended(&device, GAVE_UP, now);
    }
    assert_eq!(pacing.ended(&device, WakeOutcome::Delivered, now), None);
    assert_eq!(pacing.ready(&device, Duration::ZERO, now), Ready::Now);
    assert_eq!(
        pacing.ended(&device, GAVE_UP, now),
        Some(Duration::from_secs(2))
    );
}

#[test]
fn a_refused_or_dead_send_keeps_the_backoff_it_found() {
    let pacing = Pacing::default();
    let device = phone();
    let now = Instant::now();
    pacing.ended(&device, GAVE_UP, now);
    for outcome in [
        WakeOutcome::Refused("SENDER_ID_MISMATCH"),
        WakeOutcome::DeadToken("UNREGISTERED"),
        WakeOutcome::Cancelled,
    ] {
        assert_eq!(pacing.ended(&device, outcome, now), None);
    }
    assert_eq!(
        pacing.ended(&device, GAVE_UP, now),
        Some(Duration::from_secs(4)),
        "the run of failures continues"
    );
}

#[test]
fn the_gap_and_the_backoff_each_hold_a_device_back() {
    let pacing = Pacing::default();
    let device = phone();
    let now = Instant::now();
    assert_eq!(pacing.ready(&device, GAP, now), Ready::Now, "never sent");
    pacing.started(device.device_id);
    assert_eq!(pacing.ready(&device, GAP, now), Ready::InFlight);
    pacing.ended(&device, WakeOutcome::Delivered, now);
    assert_eq!(pacing.ready(&device, GAP, now), Ready::At(now + GAP));
    pacing.ended(&device, GAVE_UP, now);
    assert_eq!(
        pacing.ready(&device, Duration::ZERO, now),
        Ready::At(now + Duration::from_secs(2)),
        "the backoff when it is longer than the gap"
    );
    assert_eq!(
        pacing.ready(&device, GAP, now),
        Ready::At(now + GAP),
        "the gap when it is longer than the backoff"
    );
}

#[test]
fn pruning_keeps_a_send_in_flight_however_long_it_runs() {
    let pacing = Pacing::default();
    let device = phone();
    let now = Instant::now();
    pacing.started(device.device_id);
    pacing.prune(GAP, now + GAP * 100);
    assert_eq!(pacing.ready(&device, GAP, now), Ready::InFlight);

    pacing.ended(&device, WakeOutcome::Delivered, now);
    pacing.prune(GAP, now + GAP);
    assert!(
        pacing.lock().is_empty(),
        "an ended send is kept for its gap"
    );
}

#[test]
fn pruning_keeps_a_backoff_for_ten_minutes_past_its_end() {
    let pacing = Pacing::default();
    let device = phone();
    let now = Instant::now();
    pacing.ended(&device, GAVE_UP, now);
    let until = now + Duration::from_secs(2);
    pacing.prune(GAP, until + MAX_RETRY_DELAY - Duration::from_millis(1));
    assert_eq!(pacing.lock().len(), 1);
    pacing.prune(GAP, until + MAX_RETRY_DELAY);
    assert!(pacing.lock().is_empty());
}

#[test]
fn a_send_left_in_flight_by_a_dead_dispatcher_ends_when_the_next_starts() {
    let pacing = Pacing::default();
    let device = phone();
    let started = Instant::now();
    pacing.started(device.device_id);
    let restarted = started + GAP * 3;
    pacing.orphans_ended(restarted);
    assert_eq!(
        pacing.ready(&device, GAP, restarted),
        Ready::At(restarted + GAP),
        "the gap runs from the restart, not from when the send started"
    );
}
