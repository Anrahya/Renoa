//! Plugin setup links reach the person privately, never through the RCP task
//! journal: a credential-setup link carries the key that keeps a relayed
//! credential unreadable to the coordinator.
//!
//! A command from Discord gets its links by Discord DM. A command from
//! anywhere else gets each link as a notice sealed to every signed-in app
//! device of the owner; the coordinator holds only the ciphertext. A link that
//! fails the check or reaches no one stops the turn with the reason. When a
//! turn stops, every `plugin_manage` call whose link did not reach the person
//! has its notices withdrawn and its saved setup forgotten, so no follow-up
//! comes back about a link no one had.

mod seal;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture};
use renoa_discord::{OperatorChannel, SetupDelivery};
use renoa_kernel::{CommandId, SessionId};
use renoa_local::{LocalHost, RenoaHome, SetupLink};
use renoa_rcp_wire::{DeviceNoticeKey, SealedNotice};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{agent_link::AgentLink, node_log};

/// The surface whose commands get their setup links by Discord DM.
const DISCORD_SURFACE: &str = "discord";
/// The tool whose calls show setup links.
const SETUP_TOOL: &str = "plugin_manage";

/// Where one command's setup links go.
enum Route {
    Discord(SetupDelivery),
    App(Arc<AgentLink>),
    /// No private place is open; each link stops the turn with this reason.
    Closed(String),
}

/// The setup links one command's turn shows, delivered as they appear.
pub(crate) struct SetupLinks {
    route: Route,
    command: Uuid,
    cancellation: CancellationToken,
    /// Held only to record a result, never across an await.
    outcome: Mutex<Outcome>,
}

/// A turn that stopped before every setup link reached the person.
pub(crate) struct Stopped {
    /// Why a link stopped the turn; `None` when the turn was stopped for
    /// another reason.
    reason: Option<String>,
    /// The `plugin_manage` calls whose link did not reach the person.
    calls: Vec<String>,
}

impl Stopped {
    /// Forgets the setups the undelivered calls saved, so none comes back as
    /// a follow-up about a link no one had, and returns why a link stopped
    /// the turn. A replay already waiting on one ends at its deadline.
    pub(crate) async fn forget_setups(
        self,
        host: &LocalHost,
        session: Uuid,
        command: Uuid,
    ) -> Option<String> {
        for call in &self.calls {
            let forgotten = host
                .forget_plugin_setup(
                    SessionId::from_uuid(session),
                    CommandId::from_uuid(command),
                    call,
                )
                .await;
            if let Err(error) = forgotten {
                log_failure(command, "setup_forget_failed", &error.to_string());
            }
        }
        self.reason
    }
}

#[derive(Default)]
struct Outcome {
    /// Why the first undelivered link stopped the turn.
    failure: Option<String>,
    /// Each `plugin_manage` call of the turn, by tool call id.
    calls: BTreeMap<String, Call>,
}

/// One `plugin_manage` call's links.
#[derive(Default)]
struct Call {
    /// Every link it showed reached the person, and it showed one.
    delivered: bool,
    /// One of its links did not; the call cannot finish.
    failed: bool,
    /// The notices put for its links.
    notices: Vec<Uuid>,
}

impl SetupLinks {
    /// The delivery for command `command`, which `surface` submitted.
    pub(crate) fn open(
        home: &RenoaHome,
        link: &Arc<AgentLink>,
        surface: &str,
        command: Uuid,
        cancellation: CancellationToken,
    ) -> Self {
        let route = if surface == DISCORD_SURFACE {
            match OperatorChannel::open(home) {
                Ok(Some(channel)) => Route::Discord(channel.setup_delivery(command, cancellation.clone())),
                Ok(None) => Route::Closed(
                    "Discord is not connected to this Renoa, so the setup link has nowhere private to go. Connect Discord or use the Renoa app, then ask again.".to_owned(),
                ),
                Err(error) => {
                    log_failure(command, "operator_channel_unavailable", &error.to_string());
                    Route::Closed(format!(
                        "The Discord connection could not be read ({error}), so no setup link was sent. Ask again once Discord works."
                    ))
                }
            }
        } else {
            Route::App(Arc::clone(link))
        };
        Self {
            route,
            command,
            cancellation,
            outcome: Mutex::new(Outcome::default()),
        }
    }

    /// Ends the command's delivery: `None` when its turn was not stopped, or
    /// when every link it showed reached the person. Otherwise the notices of
    /// each call whose link did not are withdrawn, once; a withdrawal that
    /// fails leaves them to expire with their links.
    pub(crate) async fn finish(&self) -> Option<Stopped> {
        if !self.cancellation.is_cancelled() {
            return None;
        }
        let (reason, calls, notices) = {
            let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
            let mut calls = Vec::new();
            let mut notices = Vec::new();
            for (id, call) in std::mem::take(&mut outcome.calls) {
                if call.failed || !call.delivered {
                    calls.push(id);
                    notices.extend(call.notices);
                }
            }
            (outcome.failure.take(), calls, notices)
        };
        if reason.is_none() && calls.is_empty() {
            return None;
        }
        if let Route::App(link) = &self.route
            && !notices.is_empty()
            && let Err(error) = link.withdraw_notices(notices).await
        {
            log_failure(self.command, "setup_notice_withdraw_failed", &error);
        }
        Some(Stopped { reason, calls })
    }

    /// Records a `plugin_manage` call as it starts, so a stopped turn can
    /// forget its setup even when its link never arrived.
    fn started(&self, call: &str) {
        self.outcome
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .entry(call.to_owned())
            .or_default();
    }

    /// Delivers `link`; `Ok(false)` when the turn was stopped first.
    async fn deliver(&self, call: &str, link: &SetupLink) -> Result<bool, String> {
        if self.cancellation.is_cancelled() {
            return Ok(false);
        }
        match &self.route {
            Route::Discord(delivery) => delivery
                .deliver(call, link)
                .await
                .map(|()| !self.cancellation.is_cancelled())
                .map_err(|error| error.to_string()),
            Route::App(coordinator) => self
                .put_notices(coordinator, call, link)
                .await
                .map(|()| true),
            Route::Closed(reason) => Err(reason.clone()),
        }
    }

    /// Seals `link` to each of the owner's signed-in devices and puts the
    /// notices. A device whose key cannot be sealed to, or whose notice the
    /// coordinator refuses, is skipped; the link fails only when no device
    /// holds it.
    async fn put_notices(
        &self,
        coordinator: &AgentLink,
        call: &str,
        link: &SetupLink,
    ) -> Result<(), String> {
        let expires_at_ms = link.expires_at_ms.ok_or(
            "This setup link has no expiry, so the Renoa app cannot hold it. Ask again to restart the setup.",
        )?;
        let keys = coordinator.notice_keys().await.map_err(|error| {
            format!("The Renoa coordinator did not list your signed-in apps ({error}), so no setup link was sent. Ask again.")
        })?;
        let plaintext = plaintext(link, expires_at_ms)?;
        let mut put = 0_usize;
        let mut refused = None;
        for DeviceNoticeKey {
            device_id,
            public_key,
        } in keys
        {
            let notice_id = notice_id(self.command, call, link, device_id.as_uuid());
            let sealed = public_key.point().and_then(|point| {
                seal::seal(&point, notice_id, device_id.as_uuid(), &plaintext).ok()
            });
            let Some(sealed) = sealed else {
                log_failure(
                    self.command,
                    "setup_notice_unsealable",
                    &format!("device {device_id} has no usable notice key"),
                );
                continue;
            };
            // Recorded before the put, whose outcome may stay unknown, so a
            // stopped turn withdraws it either way.
            self.outcome
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .calls
                .entry(call.to_owned())
                .or_default()
                .notices
                .push(notice_id);
            let held = coordinator
                .put_notice(SealedNotice {
                    notice_id,
                    device_id,
                    sealed,
                    expires_at_ms,
                })
                .await;
            match held {
                Ok(()) => put += 1,
                Err(error) => {
                    log_failure(self.command, "setup_notice_put_failed", &error);
                    refused.get_or_insert(error);
                }
            }
        }
        match (put, refused) {
            (0, Some(error)) => Err(format!(
                "The setup link could not be held for your Renoa app ({error}). Ask again."
            )),
            (0, None) => Err("No signed-in Renoa app can receive this setup link. Sign in to the Renoa app or connect Discord, then ask again.".to_owned()),
            _ => Ok(()),
        }
    }
}

impl AgentEventSink for SetupLinks {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let (call, update) = match event {
                AgentEvent::ToolExecutionStart { call } if call.name == SETUP_TOOL => {
                    self.started(&call.id);
                    return;
                }
                AgentEvent::ToolExecutionUpdate { call, update } => (call, update),
                _ => return,
            };
            let delivered = match SetupLink::from_update(&call.name, &update) {
                None => return,
                Some(Ok(link)) => self.deliver(&call.id, &link).await,
                Some(Err(error)) => Err(format!(
                    "{error}, so no link was sent. Ask again to restart the setup."
                )),
            };
            {
                let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
                match &delivered {
                    Ok(reached) => outcome.calls.entry(call.id).or_default().delivered |= reached,
                    Err(reason) => {
                        outcome.failure.get_or_insert_with(|| reason.clone());
                        outcome.calls.entry(call.id).or_default().failed = true;
                    }
                }
            }
            if delivered.is_err() {
                self.cancellation.cancel();
            }
        })
    }
}

/// What a device opens: the link and what it connects, as JSON.
fn plaintext(link: &SetupLink, expires_at_ms: i64) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&serde_json::json!({
        "kind": link.kind.as_str(),
        "label": link.label,
        "url": link.url.as_str(),
        "expires_at_ms": expires_at_ms,
    }))
    .map_err(|error| error.to_string())
}

fn log_failure(command: Uuid, event: &'static str, error: &str) {
    node_log::event(
        "warn",
        event,
        &serde_json::json!({ "command_id": command, "error": error }),
    );
}

/// The notice of one link of one tool call for one device: the same on every
/// retry, so a repeated put converges on the stored notice, and another link
/// is another notice.
fn notice_id(command: Uuid, call: &str, link: &SetupLink, device: Uuid) -> Uuid {
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    for part in [
        command.as_bytes().as_slice(),
        call.as_bytes(),
        link.kind.as_str().as_bytes(),
        link.url.as_str().as_bytes(),
        device.as_bytes().as_slice(),
    ] {
        hash.update(&u64::try_from(part.len()).unwrap_or(u64::MAX).to_le_bytes());
        hash.update(part);
    }
    let digest = hash.finish();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_ref()[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

#[cfg(test)]
#[path = "setup/tests.rs"]
mod tests;
