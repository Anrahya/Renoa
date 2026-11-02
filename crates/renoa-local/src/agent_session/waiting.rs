//! A surface's turn or compaction waiting for its claim on the session while a
//! `renoa.memory` note pass lets go of it (see [`claim`](super::claim)).

use renoa_kernel::CommandId;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    AgentSession,
    claim::{Claim, Waited},
};
use crate::{LocalHostError, host::automations, plugins::host::memory::note_pass};

impl AgentSession {
    /// Claims this session for the surface operation `request`, cancelling a
    /// note pass that holds it and waiting for the pass to let go. While it
    /// waits, [`Self::cancel_active_turn`] cancels `cancellation` too.
    ///
    /// Cancelled while it waits, a command the kernel never admitted returns
    /// `None`, with nothing claimed or run. One the kernel admitted settles
    /// only under the claim, so it waits on, uncancelled, and the caller
    /// settles it as cancelled. An agent's automation run the scheduler is
    /// delivering waits again each time the bound passes, under its own
    /// identity, and so does one whose delivery cannot be read, so a pass
    /// slow to stop does not fail it; any other operation is refused, to be
    /// sent again.
    ///
    /// # Errors
    ///
    /// Returns [`LocalHostError::PassStillRunning`] past the bound, or a
    /// journal failure.
    pub(super) async fn surface_claim(
        &self,
        request: Uuid,
        cancellation: &CancellationToken,
    ) -> Result<Option<Claim>, LocalHostError> {
        let _waiting = Waiting::register(self, cancellation)?;
        let uncancelled = CancellationToken::new();
        let mut settling = false;
        loop {
            let watched = if settling { &uncancelled } else { cancellation };
            match self.host.claims.surface(self.id, watched).await {
                Ok(claim) => return Ok(Some(claim)),
                Err(Waited::Cancelled) if self.admitted(request)? => settling = true,
                Err(Waited::Cancelled) => return Ok(None),
                Err(Waited::TimedOut) if settling => {
                    note_pass::delays_command(self.id, request, "cancelled after admission");
                }
                Err(Waited::TimedOut) if self.delivering(request).await => {
                    note_pass::delays_command(self.id, request, "automation run");
                }
                Err(Waited::TimedOut) => return Err(LocalHostError::PassStillRunning),
            }
        }
    }

    /// Whether the kernel admitted `request`.
    fn admitted(&self, request: Uuid) -> Result<bool, LocalHostError> {
        Ok(self
            .kernel
            .command_standing(CommandId::from_uuid(request))?
            .admitted)
    }

    /// Whether `request` is an agent's automation run the scheduler is
    /// delivering. A catalog that cannot be read counts as delivering, and
    /// is logged, so a reminder is not failed for it.
    async fn delivering(&self, request: Uuid) -> bool {
        let database = self.host.database.clone();
        let read = tokio::task::spawn_blocking(move || automations::delivering(&database, request))
            .await
            .map_err(LocalHostError::from)
            .and_then(|read| read.map_err(LocalHostError::from));
        read.unwrap_or_else(|error| {
            note_pass::delivery_unread(self.id, request, &error);
            true
        })
    }
}

/// One waiting operation's cancellation, which [`AgentSession::cancel_active_turn`]
/// reaches until it is dropped.
struct Waiting<'a> {
    session: &'a AgentSession,
    id: Uuid,
}

impl<'a> Waiting<'a> {
    fn register(
        session: &'a AgentSession,
        cancellation: &CancellationToken,
    ) -> Result<Self, LocalHostError> {
        let id = Uuid::new_v4();
        session.state()?.waiting.insert(id, cancellation.clone());
        Ok(Self { session, id })
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.session.state() {
            state.waiting.remove(&self.id);
        }
    }
}
