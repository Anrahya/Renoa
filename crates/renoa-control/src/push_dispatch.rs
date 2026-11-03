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
//! marker is settled at that generation only, so a wake recorded during a send
//! stays owed and is sent again. A marker's generation never restarts, so a
//! send started for an older wake cannot cover a newer one. No SQLite lock is
//! held across a network call.
//!
//! While a ring is owed, a device's send rings unless the device already had
//! a send that counts started at or after the ring's generation; the first
//! such send to each device's current token is the ring, so a device rings
//! once and a wake that follows does not ring it again. A token registered
//! while the ring is owed rings too, and a restart forgets which devices
//! rang, so a ring still owed rings each device again.
//!
//! Wakes coalesce three ways: one marker per principal, a least gap between
//! two sends to one device (the marker stays until the trailing wake goes
//! out), and FCM's collapse key, so an offline phone holds one pending wake.
//! A wake is sent at least once while its marker is owed: a shutdown or
//! restart mid-send leaves it owed, and the next `serve` sends again. A send
//! that gave up after its retries leaves its device owed, sent again after
//! the backoff [`Pacing`] keeps. A wake FCM refuses or whose token it calls
//! dead counts as sent and is not repeated; the journal is the truth and the
//! app reconciles from its cursors. A device revoked before a send starts is
//! not sent one; a send already handed to FCM cannot be recalled.

use std::{collections::HashMap, sync::Arc, time::Duration};

use renoa_protocol::PrincipalId;
use tokio::{
    task::{self, JoinError, JoinHandle, JoinSet},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    ControlError, DeviceId, PushToken, control_log,
    coordinator::CoordinatorState,
    fcm::{FcmSender, WakeKind, WakeOutcome},
    push_pacing::{Pacing, Ready},
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

/// A send in flight.
struct Sending {
    principal_id: PrincipalId,
    /// The marker's generation when the send started.
    generation: i64,
    target: PushTarget,
    kind: WakeKind,
}

/// The latest send to one device that delivered, or that FCM refused or
/// called its token dead.
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
    pacing: Pacing,
    /// Per principal with an owed wake, each device's latest send that counts.
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
    pacing: Pacing,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(dispatch(state, delivery, pacing, shutdown))
}

/// Runs a dispatcher until `shutdown`.
pub(crate) async fn dispatch(
    state: Arc<CoordinatorState>,
    delivery: Delivery,
    pacing: Pacing,
    shutdown: CancellationToken,
) {
    pacing.orphans_ended(Instant::now());
    let dispatcher = Dispatcher {
        state,
        pacing,
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
                    self.settle(wake.principal_id, wake.generation).await
                }
            };
            if let Err(error) = result {
                // Keep what the other wakes made due, and come back for this one.
                storage_failed(Some(wake.principal_id), &error);
                retry_soon(&mut due);
            }
        }
        if let Delivery::Fcm { gap, .. } = delivery {
            self.pacing.prune(*gap, Instant::now());
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
        let mut waiting = false;
        let mut ready = Vec::new();
        for target in targets {
            let had = covered.get(&target.device_id);
            if had.is_some_and(|covered| covered.covers(&target, wake.generation)) {
                continue;
            }
            waiting = true;
            let kind = match wake.ring {
                Some(ring) if !had.is_some_and(|covered| covered.covers(&target, ring)) => {
                    WakeKind::Ring
                }
                Some(_) | None => WakeKind::Changed,
            };
            ready.push((target, kind));
        }
        for (target, kind) in ready {
            match self.pacing.ready(&target, gap, Instant::now()) {
                Ready::At(at) => earliest(due, at),
                Ready::Now if self.in_flight.len() < MAX_IN_FLIGHT => {
                    self.start(sender, shutdown, wake, target, kind);
                }
                // At the bound, the send that ends next starts another pass;
                // a send in flight starts one when it ends.
                Ready::Now | Ready::InFlight => {}
            }
        }
        if waiting {
            return Ok(());
        }
        self.settle(wake.principal_id, wake.generation).await
    }

    fn start(
        &mut self,
        sender: &Arc<FcmSender>,
        shutdown: &CancellationToken,
        wake: OwedWake,
        target: PushTarget,
        kind: WakeKind,
    ) {
        self.pacing.started(target.device_id);
        let sender = Arc::clone(sender);
        let stop = shutdown.clone();
        let token = target.token.clone();
        let handle = self
            .in_flight
            .spawn(async move { sender.wake(&token, kind, &stop).await });
        self.sending.insert(
            handle.id(),
            Sending {
                principal_id: wake.principal_id,
                generation: wake.generation,
                target,
                kind,
            },
        );
    }

    /// Records a send that ended, reports it, and settles its wake when every
    /// device of the principal has had a send that counts.
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
            kind,
        }) = self.sending.remove(&id)
        else {
            return Ok(());
        };
        let device_id = target.device_id;
        // The gap runs from the end of the send, whatever its outcome, so a
        // failing device sees no more requests than a healthy one.
        let retry_after = self.pacing.ended(&target, outcome, Instant::now());
        match outcome {
            WakeOutcome::Cancelled => return Ok(()),
            // The device stays owed, and a pass sends again after its backoff.
            WakeOutcome::GaveUp(_) => {
                return report(
                    &self.state,
                    principal_id,
                    target,
                    kind,
                    outcome,
                    retry_after,
                )
                .await;
            }
            WakeOutcome::Delivered | WakeOutcome::DeadToken(_) | WakeOutcome::Refused(_) => {}
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
        report(&self.state, principal_id, target, kind, outcome, None).await?;
        self.settle_if_covered(principal_id, generation).await
    }

    /// Settles the principal's marker once every device it would wake has had
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
        self.settle(principal_id, generation).await
    }

    async fn settle(
        &mut self,
        principal_id: PrincipalId,
        generation: i64,
    ) -> Result<(), ControlError> {
        if self
            .state
            .store
            .settle_wake(principal_id, generation)
            .await?
        {
            self.covered.remove(&principal_id);
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
    kind: WakeKind,
    outcome: WakeOutcome,
    retry_after: Option<Duration>,
) -> Result<(), ControlError> {
    let device_id = target.device_id;
    let fields = |reason: &str| {
        serde_json::json!({
            "device_id": device_id,
            "principal_id": principal_id,
            "kind": kind.as_str(),
            "reason": reason,
            "retry_after_ms": retry_after.map(|delay| delay.as_millis()),
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
