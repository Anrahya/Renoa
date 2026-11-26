//! Setup notices on one session: registers this device's notice key on each
//! connection (retrying on later refreshes after a transient failure), drives a
//! single race-free per-connection worker via `tokio::sync::Notify`, opens held
//! notices, acknowledges unreadable ones, and serializes reads against owner
//! acknowledgements so a dismissed card cannot reappear from an in-flight list.

use std::sync::Arc;

use renoa_rcp_client::{ClientError, Connection};
use renoa_rcp_wire::DeviceId;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use uuid::Uuid;

use super::{Update, session::within};
use crate::notice::{HeldLink, NoticeSecret, now_ms};

#[derive(Default)]
struct State {
    registered_on: Option<u64>,
    held: Vec<HeldLink>,
}

#[derive(Clone)]
pub(super) struct Notices {
    device_id: DeviceId,
    key: Arc<NoticeSecret>,
    state: Arc<Mutex<State>>,
}

impl Notices {
    pub(super) fn new(device_id: DeviceId, key: Arc<NoticeSecret>) -> Self {
        Self {
            device_id,
            key,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Long-lived per-connection worker: registers the notice key and reads
    /// waiting notices whenever `refresh` is notified (including once on start).
    /// Because `Notify` stores a single permit across concurrent notifications,
    /// refreshes are coalesced without losing one at task shutdown.
    pub(super) async fn watch(
        self,
        connection_id: u64,
        connection: Connection,
        refresh: Arc<Notify>,
        updates: mpsc::UnboundedSender<Update>,
    ) {
        refresh.notify_one();
        loop {
            refresh.notified().await;
            let mut state = self.state.lock().await;
            if state.registered_on != Some(connection_id) {
                if within(connection.set_notice_key(self.key.public_key()))
                    .await
                    .is_err()
                {
                    continue;
                }
                state.registered_on = Some(connection_id);
            }
            self.read_locked(&connection, &updates, &mut state.held)
                .await;
        }
    }

    /// Acknowledges `notice_id` under the same lock as [`Self::watch`], removing
    /// it from the shown list on success and reporting whether it succeeded.
    pub(super) async fn acknowledge(
        self,
        connection: Connection,
        notice_id: Uuid,
        updates: mpsc::UnboundedSender<Update>,
        answered: oneshot::Sender<bool>,
    ) {
        let mut state = self.state.lock().await;
        let ok = within(connection.acknowledge_notice(notice_id))
            .await
            .is_ok();
        if ok {
            state.held.retain(|item| item.notice_id != notice_id);
            let _ = updates.send(Update::SetupLinks(state.held.clone()));
        }
        let _ = answered.send(ok);
    }

    async fn read_locked(
        &self,
        connection: &Connection,
        updates: &mpsc::UnboundedSender<Update>,
        held: &mut Vec<HeldLink>,
    ) {
        let Ok(listed) = within(connection.list_notices()).await else {
            return;
        };
        let now = now_ms();
        let mut opened = Vec::with_capacity(listed.len());
        let mut transport_ok = true;
        for notice in listed {
            match self.key.open(&notice, self.device_id) {
                Ok(link) if link.expires_at_ms > now => opened.push(HeldLink {
                    notice_id: notice.notice_id,
                    link,
                }),
                Ok(_) => {}
                // A notice this device cannot open never will open: clear it,
                // stopping early if the connection drops or times out.
                Err(_) if transport_ok => {
                    if let Err(ClientError::Transport(_)) =
                        within(connection.acknowledge_notice(notice.notice_id)).await
                    {
                        transport_ok = false;
                    }
                }
                Err(_) => {}
            }
        }
        if *held != opened {
            *held = opened;
            let _ = updates.send(Update::SetupLinks(held.clone()));
        }
    }
}
