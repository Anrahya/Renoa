//! Draining owed wakes to the owner's devices.
//!
//! `Coordinator::serve` starts the dispatcher with its shutdown token, so the
//! one-shot commands that only open the coordinator start no background work.
//! A pass reads each principal's owed wake and its generation, sends to each
//! of the principal's registered devices that are not revoked, and deletes the
//! marker at that generation; a wake recorded meanwhile survives for the next
//! pass. No SQLite lock is held across a network call.
//!
//! Wakes coalesce three ways: one marker per principal, a least gap between
//! two sends to one device (the marker stays until the trailing wake goes
//! out), and FCM's collapse key, so an offline phone holds one pending wake.
//! A wake is a hint, not a delivery: one lost to an FCM outage or a restart
//! mid-send is not repeated, and the app reconciles from the journal.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use renoa_protocol::PrincipalId;
use tokio::{task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    ControlError, DeviceId, control_log,
    coordinator::CoordinatorState,
    fcm::{FcmSender, WakeOutcome},
    push_tokens::PushTarget,
    push_wake::OwedWake,
};

/// Where wakes go: FCM, or nowhere when no service account is configured.
pub(crate) enum Delivery {
    /// Boxed: the sender is large and built once.
    Fcm {
        sender: Box<FcmSender>,
        gap: Duration,
    },
    Disabled,
}

/// The devices that already received the wake owed at one generation.
struct Covered {
    generation: i64,
    devices: HashSet<DeviceId>,
}

struct Dispatcher {
    state: Arc<CoordinatorState>,
    /// When each device was last sent a wake; entries older than the gap are
    /// pruned after every pass.
    last_sent: HashMap<DeviceId, Instant>,
    /// Per principal with an owed wake; removed when its marker is deleted.
    covered: HashMap<PrincipalId, Covered>,
    disabled_reported: bool,
}

pub(crate) fn spawn(
    state: Arc<CoordinatorState>,
    delivery: Delivery,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    let dispatcher = Dispatcher {
        state,
        last_sent: HashMap::new(),
        covered: HashMap::new(),
        disabled_reported: false,
    };
    tokio::spawn(dispatcher.run(delivery, shutdown))
}

impl Dispatcher {
    async fn run(mut self, delivery: Delivery, shutdown: CancellationToken) {
        loop {
            // A notification that arrives during a pass is kept by `Notify`
            // and starts the next one.
            let due = match self.pass(&delivery, &shutdown).await {
                Ok(due) => due,
                Err(error) => {
                    control_log::event(
                        "error",
                        "push_wake_failed",
                        &serde_json::json!({ "reason": "storage", "error": error.to_string() }),
                    );
                    None
                }
            };
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                () = self.state.push_wakes.notified() => {}
                () = tokio::time::sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {}
            }
        }
    }

    /// Drains every owed wake it can. Returns when the earliest deferred one
    /// falls due.
    async fn pass(
        &mut self,
        delivery: &Delivery,
        shutdown: &CancellationToken,
    ) -> Result<Option<Instant>, ControlError> {
        let mut due: Option<Instant> = None;
        for wake in self.state.store.owed_wakes().await? {
            if shutdown.is_cancelled() {
                break;
            }
            let deferred = match delivery {
                Delivery::Fcm { sender, gap } => self.deliver(sender, *gap, wake, shutdown).await?,
                Delivery::Disabled => {
                    self.report_disabled(wake.principal_id);
                    None
                }
            };
            if let Some(at) = deferred {
                due = Some(due.map_or(at, |earliest| earliest.min(at)));
            } else {
                self.state.store.settle_wake(wake).await?;
                self.covered.remove(&wake.principal_id);
            }
        }
        if let Delivery::Fcm { gap, .. } = delivery {
            let now = Instant::now();
            self.last_sent.retain(|_, sent| now < *sent + *gap);
        }
        Ok(due)
    }

    fn report_disabled(&mut self, principal_id: PrincipalId) {
        if self.disabled_reported {
            return;
        }
        self.disabled_reported = true;
        control_log::event(
            "info",
            "push_disabled",
            &serde_json::json!({
                "principal_id": principal_id,
                "reason": "no FCM service account is configured; owed wakes are dropped",
            }),
        );
    }

    /// Sends one owed wake to every device of its principal that has not had
    /// it. Returns when a device inside its gap falls due, or `None` once
    /// every device is covered.
    async fn deliver(
        &mut self,
        sender: &FcmSender,
        gap: Duration,
        wake: OwedWake,
        shutdown: &CancellationToken,
    ) -> Result<Option<Instant>, ControlError> {
        let targets = self.state.store.push_targets(wake.principal_id).await?;
        let covered = self.covered.entry(wake.principal_id).or_insert(Covered {
            generation: wake.generation,
            devices: HashSet::new(),
        });
        if covered.generation != wake.generation {
            *covered = Covered {
                generation: wake.generation,
                devices: HashSet::new(),
            };
        }
        let mut due: Option<Instant> = None;
        for target in targets {
            if covered.devices.contains(&target.device_id) {
                continue;
            }
            let now = Instant::now();
            if let Some(sent) = self.last_sent.get(&target.device_id)
                && now < *sent + gap
            {
                let at = *sent + gap;
                due = Some(due.map_or(at, |earliest| earliest.min(at)));
                continue;
            }
            let device_id = target.device_id;
            let outcome = sender.wake(&target.token, shutdown).await;
            // The gap runs from the end of the send, whatever its outcome, so
            // a failing device sees no more requests than a healthy one.
            self.last_sent.insert(device_id, Instant::now());
            if outcome == WakeOutcome::Cancelled {
                return Ok(Some(Instant::now()));
            }
            report(&self.state, wake.principal_id, target, outcome).await?;
            covered.devices.insert(device_id);
        }
        Ok(due)
    }
}

async fn report(
    state: &CoordinatorState,
    principal_id: PrincipalId,
    target: PushTarget,
    outcome: WakeOutcome,
) -> Result<(), ControlError> {
    let device_id = target.device_id;
    let (level, name, reason) = match outcome {
        WakeOutcome::Delivered => ("info", "push_wake_sent", "delivered"),
        WakeOutcome::DeadToken(reason) => {
            state.store.drop_push_token(target).await?;
            ("info", "push_token_dropped", reason)
        }
        WakeOutcome::Refused(reason) | WakeOutcome::GaveUp(reason) => {
            ("warn", "push_wake_failed", reason)
        }
        WakeOutcome::Cancelled => return Ok(()),
    };
    control_log::event(
        level,
        name,
        &serde_json::json!({
            "device_id": device_id,
            "principal_id": principal_id,
            "reason": reason,
            "retried": matches!(outcome, WakeOutcome::GaveUp(_)),
        }),
    );
    Ok(())
}
