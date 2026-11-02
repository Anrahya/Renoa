//! Draining owed wakes to the owner's devices.
//!
//! `Coordinator::serve` runs the dispatcher under
//! [`push_supervisor::supervise`](crate::push_supervisor::supervise), so the
//! one-shot commands that only open the coordinator start no background work.
//! A pass reads each principal's owed wake and its generation, and starts a
//! send to each of the principal's registered, unrevoked devices that has not
//! had a send to its current token since that generation. At most
//! [`MAX_IN_FLIGHT`] sends run at once across every principal, so a device
//! whose sends hang or are retried holds up no other device, up to that bound.
//! Once every device has had a send that started at or after a generation, the
//! marker is deleted at that generation only, so a wake recorded during a send
//! stays owed and is sent again. No SQLite lock is held across a network call.
//!
//! Wakes coalesce three ways: one marker per principal, a least gap between
//! two sends to one device (the marker stays until the trailing wake goes
//! out), and FCM's collapse key, so an offline phone holds one pending wake.
//! A wake is sent at least once while its marker exists: a shutdown or restart
//! mid-send leaves the marker, and the next `serve` sends again. A wake FCM
//! refuses, or one that outlasts its retries, is not repeated; the journal is
//! the truth and the app reconciles from its cursors. A device revoked before
//! a send starts is not sent one; a send already handed to FCM cannot be
//! recalled.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use renoa_protocol::PrincipalId;
use tokio::{
    task::{self, JoinError, JoinHandle, JoinSet},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    ControlError, DeviceId, PushToken, control_log,
    coordinator::CoordinatorState,
    fcm::{FcmSender, WakeOutcome},
    push_tokens::PushTarget,
    push_wake::OwedWake,
};

/// The most sends in flight at once, across every principal.
const MAX_IN_FLIGHT: usize = 8;
/// How soon a pass runs again after a storage failure.
const STORAGE_RETRY: Duration = Duration::from_secs(5);

/// Where wakes go: FCM, or nowhere when no service account is configured.
#[derive(Clone)]
pub(crate) enum Delivery {
    Fcm {
        sender: Arc<FcmSender>,
        gap: Duration,
    },
    Disabled,
}

/// When each device's latest send started or ended. The supervisor keeps it
/// across dispatcher restarts, so a dispatcher that dies mid-send does not
/// cost a device its gap. It lives in memory only: a coordinator restart
/// forgets it, so the first wake after one may come inside a device's gap.
#[derive(Clone, Default)]
pub(crate) struct LastSent(Arc<Mutex<HashMap<DeviceId, Instant>>>);

impl LastSent {
    fn lock(&self) -> MutexGuard<'_, HashMap<DeviceId, Instant>> {
        // A dispatcher that panicked while holding it left whole entries.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A send in flight.
struct Sending {
    principal_id: PrincipalId,
    /// The marker's generation when the send started.
    generation: i64,
    target: PushTarget,
}

/// The latest ended send to one device.
struct Covered {
    /// The marker's generation when the send started.
    generation: i64,
    /// The token it went to; a device registered with another since is not
    /// covered.
    token: PushToken,
}

impl Covered {
    fn covers(&self, target: &PushTarget, generation: i64) -> bool {
        self.generation >= generation && self.token == target.token
    }
}

struct Dispatcher {
    state: Arc<CoordinatorState>,
    last_sent: LastSent,
    /// Per principal with an owed wake, each device's latest ended send.
    /// Dropped with the principal's marker.
    covered: HashMap<PrincipalId, HashMap<DeviceId, Covered>>,
    in_flight: JoinSet<WakeOutcome>,
    sending: HashMap<task::Id, Sending>,
    disabled_reported: bool,
}

/// What ended the dispatcher's wait.
enum Woken {
    SendEnded(Result<(task::Id, WakeOutcome), JoinError>),
    Pass,
}

pub(crate) fn spawn(
    state: Arc<CoordinatorState>,
    delivery: Delivery,
    last_sent: LastSent,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(dispatch(state, delivery, last_sent, shutdown))
}

/// Runs a dispatcher until `shutdown`.
pub(crate) async fn dispatch(
    state: Arc<CoordinatorState>,
    delivery: Delivery,
    last_sent: LastSent,
    shutdown: CancellationToken,
) {
    let dispatcher = Dispatcher {
        state,
        last_sent,
        covered: HashMap::new(),
        in_flight: JoinSet::new(),
        sending: HashMap::new(),
        disabled_reported: false,
    };
    dispatcher.run(delivery, shutdown).await;
}

impl Dispatcher {
    async fn run(mut self, delivery: Delivery, shutdown: CancellationToken) {
        loop {
            let due = self.pass(&delivery, &shutdown).await;
            // A notification that arrives during a pass is kept by `Notify`
            // and starts the next one.
            let woken = tokio::select! {
                biased;
                // Sends in flight are aborted with the task; their markers stay.
                () = shutdown.cancelled() => return,
                Some(ended) = self.in_flight.join_next_with_id() => Woken::SendEnded(ended),
                () = self.state.push_wakes.notified() => Woken::Pass,
                () = tokio::time::sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {
                    Woken::Pass
                }
            };
            if let Woken::SendEnded(ended) = woken
                && let Err(error) = self.finish(ended).await
            {
                // The pass that follows settles what this could not.
                storage_failed(None, &error);
            }
        }
    }

    /// Starts every send it may and settles every wake whose sends have all
    /// ended. Returns when to run again unprompted: when a device inside its
    /// gap falls due, or soon after a storage failure.
    async fn pass(&mut self, delivery: &Delivery, shutdown: &CancellationToken) -> Option<Instant> {
        let mut due = None;
        let wakes = match self.state.store.owed_wakes().await {
            Ok(wakes) => wakes,
            Err(error) => {
                storage_failed(None, &error);
                retry_soon(&mut due);
                return due;
            }
        };
        // A marker that is gone takes its coverage with it.
        let sending = &self.sending;
        self.covered.retain(|principal_id, _| {
            wakes.iter().any(|wake| wake.principal_id == *principal_id)
                || sending
                    .values()
                    .any(|send| send.principal_id == *principal_id)
        });
        for wake in wakes {
            if shutdown.is_cancelled() {
                break;
            }
            let result = match delivery {
                Delivery::Fcm { sender, gap } => {
                    self.plan(sender, *gap, wake, shutdown, &mut due).await
                }
                Delivery::Disabled => {
                    self.report_disabled(wake.principal_id);
                    self.settle(wake).await
                }
            };
            if let Err(error) = result {
                // Keep what the other wakes made due, and come back for this one.
                storage_failed(Some(wake.principal_id), &error);
                retry_soon(&mut due);
            }
        }
        if let Delivery::Fcm { gap, .. } = delivery {
            let now = Instant::now();
            // The gap is at most a day, so adding it cannot overflow.
            self.last_sent.lock().retain(|_, sent| now < *sent + *gap);
        }
        due
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

    /// Starts a send to each device of the wake's principal that has not had
    /// one to its current token since the wake's generation, as the gap and
    /// the in-flight bound allow, and settles the wake once every device has.
    /// Moves `due` to when a device inside its gap falls due.
    async fn plan(
        &mut self,
        sender: &Arc<FcmSender>,
        gap: Duration,
        wake: OwedWake,
        shutdown: &CancellationToken,
        due: &mut Option<Instant>,
    ) -> Result<(), ControlError> {
        let targets = self.state.store.push_targets(wake.principal_id).await?;
        let covered = self.covered.entry(wake.principal_id).or_default();
        // A generation restarts at one after its marker is deleted, so
        // coverage above the marker's generation belongs to an older marker.
        if covered
            .values()
            .any(|covered| covered.generation > wake.generation)
        {
            covered.clear();
        }
        let mut waiting = false;
        let mut ready = Vec::new();
        for target in targets {
            if covered
                .get(&target.device_id)
                .is_some_and(|covered| covered.covers(&target, wake.generation))
            {
                continue;
            }
            waiting = true;
            ready.push(target);
        }
        for target in ready {
            if self.is_sending(target.device_id) {
                continue;
            }
            // The gap is at most a day, so adding it cannot overflow.
            let gap_ends = self
                .last_sent
                .lock()
                .get(&target.device_id)
                .map(|sent| *sent + gap);
            if let Some(ends) = gap_ends
                && Instant::now() < ends
            {
                earliest(due, ends);
                continue;
            }
            // At the bound, the send that ends next starts another pass.
            if self.in_flight.len() < MAX_IN_FLIGHT {
                self.start(sender, shutdown, wake, target);
            }
        }
        if waiting {
            return Ok(());
        }
        self.settle(wake).await
    }

    fn is_sending(&self, device_id: DeviceId) -> bool {
        self.sending
            .values()
            .any(|send| send.target.device_id == device_id)
    }

    fn start(
        &mut self,
        sender: &Arc<FcmSender>,
        shutdown: &CancellationToken,
        wake: OwedWake,
        target: PushTarget,
    ) {
        // Recorded at the start too, so a dispatcher that dies mid-send
        // leaves the device's gap behind for the next one.
        self.last_sent
            .lock()
            .insert(target.device_id, Instant::now());
        let sender = Arc::clone(sender);
        let stop = shutdown.clone();
        let token = target.token.clone();
        let handle = self
            .in_flight
            .spawn(async move { sender.wake(&token, &stop).await });
        self.sending.insert(
            handle.id(),
            Sending {
                principal_id: wake.principal_id,
                generation: wake.generation,
                target,
            },
        );
    }

    /// Records a send that ended, reports it, and settles its wake when every
    /// device of the principal has had a send.
    async fn finish(
        &mut self,
        ended: Result<(task::Id, WakeOutcome), JoinError>,
    ) -> Result<(), ControlError> {
        let (id, outcome) = match ended {
            Ok(ended) => ended,
            Err(error) if error.is_panic() => {
                (error.id(), WakeOutcome::Refused("the send task panicked"))
            }
            Err(error) => (error.id(), WakeOutcome::Cancelled),
        };
        let Some(Sending {
            principal_id,
            generation,
            target,
        }) = self.sending.remove(&id)
        else {
            return Ok(());
        };
        let device_id = target.device_id;
        // The gap runs from the end of the send, whatever its outcome, so a
        // failing device sees no more requests than a healthy one.
        self.last_sent.lock().insert(device_id, Instant::now());
        if outcome == WakeOutcome::Cancelled {
            return Ok(());
        }
        let devices = self.covered.entry(principal_id).or_default();
        let generation = match devices.get(&device_id) {
            Some(covered) if covered.token == target.token => covered.generation.max(generation),
            _ => generation,
        };
        devices.insert(
            device_id,
            Covered {
                generation,
                token: target.token.clone(),
            },
        );
        report(&self.state, principal_id, target, outcome).await?;
        self.settle_if_covered(principal_id, generation).await
    }

    /// Deletes the principal's marker once every device it would wake has had
    /// a send to its current token and none is in flight, at the oldest
    /// generation those sends started at. A wake recorded since keeps the
    /// marker for the next pass.
    async fn settle_if_covered(
        &mut self,
        principal_id: PrincipalId,
        ended_generation: i64,
    ) -> Result<(), ControlError> {
        if self
            .sending
            .values()
            .any(|send| send.principal_id == principal_id)
        {
            return Ok(());
        }
        let targets = self.state.store.push_targets(principal_id).await?;
        let devices = self.covered.get(&principal_id);
        let mut generation = ended_generation;
        for target in &targets {
            match devices.and_then(|devices| devices.get(&target.device_id)) {
                Some(covered) if covered.token == target.token => {
                    generation = generation.min(covered.generation);
                }
                _ => return Ok(()),
            }
        }
        self.settle(OwedWake {
            principal_id,
            generation,
        })
        .await
    }

    async fn settle(&mut self, wake: OwedWake) -> Result<(), ControlError> {
        if self.state.store.settle_wake(wake).await? {
            self.covered.remove(&wake.principal_id);
        }
        Ok(())
    }
}

fn earliest(due: &mut Option<Instant>, at: Instant) {
    *due = Some(due.map_or(at, |earliest| earliest.min(at)));
}

fn retry_soon(due: &mut Option<Instant>) {
    earliest(due, Instant::now() + STORAGE_RETRY);
}

fn storage_failed(principal_id: Option<PrincipalId>, error: &ControlError) {
    control_log::event(
        "error",
        "push_wake_failed",
        &serde_json::json!({
            "principal_id": principal_id,
            "reason": "storage",
            "error": error.to_string(),
        }),
    );
}

async fn report(
    state: &CoordinatorState,
    principal_id: PrincipalId,
    target: PushTarget,
    outcome: WakeOutcome,
) -> Result<(), ControlError> {
    let device_id = target.device_id;
    let fields = |reason: &str| {
        serde_json::json!({
            "device_id": device_id,
            "principal_id": principal_id,
            "reason": reason,
            "retried": matches!(outcome, WakeOutcome::GaveUp(_)),
        })
    };
    match outcome {
        WakeOutcome::Delivered => {
            control_log::event("info", "push_wake_sent", &fields("delivered"));
        }
        WakeOutcome::DeadToken(reason) => {
            // Logged whether or not the row could be deleted: `deleted` is
            // false when the device registered another token meanwhile, and
            // null when the deletion failed.
            let dropped = state.store.drop_push_token(target).await;
            let mut logged = fields(reason);
            logged["deleted"] = serde_json::json!(dropped.as_ref().ok());
            control_log::event("warn", "push_token_dropped", &logged);
            dropped?;
        }
        WakeOutcome::Refused(reason) | WakeOutcome::GaveUp(reason) => {
            control_log::event("warn", "push_wake_failed", &fields(reason));
        }
        WakeOutcome::Cancelled => {}
    }
    Ok(())
}
