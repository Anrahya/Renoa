//! When the dispatcher may next send to each device: not while a send to it is
//! in flight, not inside the least gap after its last send, and not before
//! the backoff after sends that gave up has passed.
//!
//! The supervisor keeps one [`Pacing`] across dispatcher restarts, so a
//! dispatcher that dies costs no device its gap or its backoff. A send left in
//! flight by a dispatcher that died is taken to have ended when the next one
//! starts. It lives in memory only: a coordinator restart forgets it, so the
//! first wake after one may come inside a device's gap or backoff.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use tokio::time::Instant;

use crate::{DeviceId, fcm::WakeOutcome};

/// The wait after a send gives up, doubling with each send in a row that
/// gives up, up to [`MAX_RETRY_DELAY`].
const FIRST_RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_RETRY_DELAY: Duration = Duration::from_mins(10);

/// Each device's pacing, shared by every dispatcher of one `serve`.
#[derive(Clone, Default)]
pub(crate) struct Pacing(Arc<Mutex<HashMap<DeviceId, Device>>>);

struct Device {
    last: LastSend,
    /// Present after a send gave up, until one is delivered.
    backoff: Option<Backoff>,
}

#[derive(Clone, Copy)]
enum LastSend {
    InFlight,
    Ended(Instant),
}

#[derive(Clone, Copy)]
struct Backoff {
    /// Sends in a row that gave up.
    failures: u32,
    /// The earliest the next send may start.
    until: Instant,
}

/// Whether a send to a device may start.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ready {
    Now,
    At(Instant),
    InFlight,
}

impl Pacing {
    fn lock(&self) -> MutexGuard<'_, HashMap<DeviceId, Device>> {
        // A dispatcher that panicked while holding it left whole entries.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Called as a dispatcher starts, when no send in flight is its own:
    /// each one left belonged to a dispatcher that died, and ends `now`.
    pub(crate) fn orphans_ended(&self, now: Instant) {
        for device in self.lock().values_mut() {
            if matches!(device.last, LastSend::InFlight) {
                device.last = LastSend::Ended(now);
            }
        }
    }

    /// Whether a send to `device_id` may start at `now`. A gap is at most a
    /// day, so adding it cannot overflow.
    pub(crate) fn ready(&self, device_id: DeviceId, gap: Duration, now: Instant) -> Ready {
        let devices = self.lock();
        let Some(device) = devices.get(&device_id) else {
            return Ready::Now;
        };
        let LastSend::Ended(ended) = device.last else {
            return Ready::InFlight;
        };
        let at = device
            .backoff
            .map_or(ended + gap, |backoff| backoff.until.max(ended + gap));
        if now < at { Ready::At(at) } else { Ready::Now }
    }

    pub(crate) fn started(&self, device_id: DeviceId) {
        self.lock()
            .entry(device_id)
            .and_modify(|device| device.last = LastSend::InFlight)
            .or_insert(Device {
                last: LastSend::InFlight,
                backoff: None,
            });
    }

    /// Records a send that ended at `now` with `outcome`. Returns how long the
    /// device now waits before it is sent again, when the send gave up.
    pub(crate) fn ended(
        &self,
        device_id: DeviceId,
        outcome: WakeOutcome,
        now: Instant,
    ) -> Option<Duration> {
        let mut devices = self.lock();
        let device = devices.entry(device_id).or_insert(Device {
            last: LastSend::Ended(now),
            backoff: None,
        });
        device.last = LastSend::Ended(now);
        match outcome {
            WakeOutcome::Delivered => {
                device.backoff = None;
                None
            }
            WakeOutcome::GaveUp(_) => {
                let failures = device
                    .backoff
                    .map_or(1, |backoff| backoff.failures.saturating_add(1));
                let delay = doubled(FIRST_RETRY_DELAY, failures, MAX_RETRY_DELAY);
                device.backoff = Some(Backoff {
                    failures,
                    until: now + delay,
                });
                Some(delay)
            }
            WakeOutcome::DeadToken(_) | WakeOutcome::Refused(_) | WakeOutcome::Cancelled => None,
        }
    }

    /// Forgets each device that constrains nothing at `now`. A device in
    /// flight is kept however long its send runs. A backoff is kept for
    /// [`MAX_RETRY_DELAY`] past its end, so a device that keeps failing
    /// keeps doubling.
    pub(crate) fn prune(&self, gap: Duration, now: Instant) {
        self.lock().retain(|_, device| {
            let LastSend::Ended(ended) = device.last else {
                return true;
            };
            now < ended + gap
                || device
                    .backoff
                    .is_some_and(|backoff| now < backoff.until + MAX_RETRY_DELAY)
        });
    }
}

/// `first` after the first failure in a row, doubling with each one up to
/// `max`.
pub(crate) fn doubled(first: Duration, failures: u32, max: Duration) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    first.saturating_mul(1 << doublings).min(max)
}

#[cfg(test)]
mod tests;
