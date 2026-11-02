//! A surface's turn or compaction waiting for its claim on the session while a
//! `renoa.memory` note pass lets go of it (see [`claim`](super::claim)).

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
    /// waits, [`Self::cancel_active_turn`] cancels `cancellation` too; `None`
    /// when it was cancelled, with nothing claimed or run, for a command the
    /// kernel never admitted. An agent's
    /// automation run the scheduler is delivering waits again each time the
    /// bound passes, under its own identity, so a pass slow to stop does not
    /// lose it; any other operation is refused, to be sent again.
    ///
    /// # Errors
    ///
    /// Returns [`LocalHostError::PassStillRunning`] past the bound, or a
    /// catalog failure.
    pub(super) async fn surface_claim(
        &self,
        request: Uuid,
        cancellation: &CancellationToken,
    ) -> Result<Option<Claim>, LocalHostError> {
        let _waiting = Waiting::register(self, cancellation)?;
        loop {
            match self.host.claims.surface(self.id, cancellation).await {
                Ok(claim) => return Ok(Some(claim)),
                // Cancelled with nothing run: honest only for a command the
                // kernel never admitted. One it did is settled only under
                // the claim, so it is refused, to be sent again.
                Err(Waited::Cancelled) => {
                    let command = renoa_kernel::CommandId::from_uuid(request);
                    return if self.kernel.command_standing(command)?.admitted {
                        Err(LocalHostError::PassStillRunning)
                    } else {
                        Ok(None)
                    };
                }
                Err(Waited::TimedOut) if self.delivering(request).await? => {
                    note_pass::delays_run(self.id, request);
                }
                Err(Waited::TimedOut) => return Err(LocalHostError::PassStillRunning),
            }
        }
    }

    /// Whether `request` is an agent's automation run the scheduler is
    /// delivering.
    async fn delivering(&self, request: Uuid) -> Result<bool, LocalHostError> {
        let database = self.host.database.clone();
        Ok(
            tokio::task::spawn_blocking(move || automations::delivering(&database, request))
                .await??,
        )
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
