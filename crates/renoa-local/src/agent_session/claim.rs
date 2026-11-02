//! Who holds an Agent session in this process, one operation at a time: its
//! surfaces, any number at once, or one `renoa.memory` note pass; and the one
//! open instance of the session that everyone in this process shares.
//!
//! The kernel lets one handle open a session at a time, so this process keeps
//! one [`AgentSession`] per session while anyone has it open, and every opener
//! (a surface's cache, a node's turn, a pass) gets that instance. A claim spans
//! one operation, not an open instance: a surface claims the session for each
//! turn, compaction or configuration change, and a pass for its review, so a
//! surface that keeps a session open between turns leaves it passable. A
//! surface that claims a session a pass holds cancels the pass and waits for
//! it, at most [`SURFACE_WAIT`] or until its own cancellation, then fails
//! without running anything; an automation run the scheduler delivers waits
//! again ([`waiting`](super::waiting)). Opening a session claims nothing and
//! cancels no pass. A pass
//! never takes a session a surface holds, and none once passes are stopped.
//! Claims do not reach another process: there the kernel's own lock refuses
//! the second opener.

use std::{
    collections::HashMap,
    future::Future,
    num::NonZeroUsize,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::Duration,
};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::AgentSession;
use crate::LocalHostError;

/// How long a surface waits for a cancelled pass to let go before it gives up.
#[cfg(not(test))]
pub(crate) const SURFACE_WAIT: Duration = Duration::from_secs(20);
/// Short in tests, which wait it out in real time.
#[cfg(test)]
pub(crate) const SURFACE_WAIT: Duration = Duration::from_secs(1);

/// Why a surface did not get its claim; nothing is claimed either way.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Waited {
    /// The pass had not let go within [`SURFACE_WAIT`].
    TimedOut,
    /// The surface's own cancellation came first.
    Cancelled,
}

/// The claims and open instances of one Host's sessions.
#[derive(Default)]
pub(crate) struct Claims {
    inner: Arc<Inner>,
    /// Each session's open instance, behind the lock its opening takes.
    open: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<Weak<AgentSession>>>>>,
}

#[derive(Default)]
struct Inner {
    held: Mutex<Held>,
    /// Signalled whenever a claim is released.
    released: Notify,
}

#[derive(Default)]
struct Held {
    holders: HashMap<Uuid, Holder>,
    /// Set once by [`Claims::stop_passes`]; no pass starts after it.
    passes_stopped: bool,
}

enum Holder {
    Surfaces(NonZeroUsize),
    /// The pass's cancellation, which the next surface cancels.
    NotePass(CancellationToken),
}

/// One holder's claim on a session, released when dropped.
pub(crate) struct Claim {
    inner: Arc<Inner>,
    session: Uuid,
}

impl Claims {
    /// Claims `session` for one operation of a surface, cancelling a note pass
    /// that holds it and waiting until that pass lets go.
    ///
    /// # Errors
    ///
    /// Refuses, with nothing claimed, when the pass has not let go within
    /// [`SURFACE_WAIT`] or `cancellation` came first. A claim free at once is
    /// taken even when `cancellation` already came, so the kernel settles a
    /// cancelled command as it stands.
    pub(crate) async fn surface(
        &self,
        session: Uuid,
        cancellation: &CancellationToken,
    ) -> Result<Claim, Waited> {
        tokio::select! {
            biased;
            claimed = tokio::time::timeout(SURFACE_WAIT, self.wait_for(session)) => {
                claimed.map_err(|_| Waited::TimedOut)
            }
            () = cancellation.cancelled() => Err(Waited::Cancelled),
        }
    }

    /// [`Self::surface`] for an operation nothing cancels.
    ///
    /// # Errors
    ///
    /// Returns [`LocalHostError::PassStillRunning`] when the pass has not let
    /// go within [`SURFACE_WAIT`].
    pub(crate) async fn surface_uncancelled(&self, session: Uuid) -> Result<Claim, LocalHostError> {
        // Nothing cancels the token, so only the bound ends the wait.
        self.surface(session, &CancellationToken::new())
            .await
            .map_err(|_| LocalHostError::PassStillRunning)
    }

    /// Whether an instance of `session` is open here for anyone but a note
    /// pass: a turn, a surface's cache, an inspection. The kernel refuses a
    /// second handle while one is, so a caller that needs its own handle is
    /// refused before its claim cancels a pass for nothing. A pass holds one
    /// reference to the instance it opened.
    pub(crate) fn open_elsewhere(&self, session: Uuid) -> bool {
        let slot = self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&session)
            .map(Arc::clone);
        let Some(open) = slot.and_then(|slot| slot.try_lock().ok().and_then(|open| open.upgrade()))
        else {
            return false;
        };
        let pass = usize::from(matches!(
            self.inner.held().holders.get(&session),
            Some(Holder::NotePass(_))
        ));
        // Less this function's own reference.
        Arc::strong_count(&open) - 1 > pass
    }

    async fn wait_for(&self, session: Uuid) -> Claim {
        loop {
            let released = self.inner.released.notified();
            tokio::pin!(released);
            // Registered before the map is read, so a release between the
            // read and the wait still wakes this surface.
            released.as_mut().enable();
            {
                let mut held = self.inner.held();
                match held.holders.get_mut(&session) {
                    None => {
                        held.holders
                            .insert(session, Holder::Surfaces(NonZeroUsize::MIN));
                        return self.claim(session);
                    }
                    Some(Holder::Surfaces(count)) => {
                        *count = count.saturating_add(1);
                        return self.claim(session);
                    }
                    Some(Holder::NotePass(cancellation)) => cancellation.cancel(),
                }
            }
            released.await;
        }
    }

    /// Claims `session` for a note pass that `cancellation` stops; `None` while
    /// anyone holds it, and once passes are stopped.
    pub(crate) fn note_pass(
        &self,
        session: Uuid,
        cancellation: &CancellationToken,
    ) -> Option<Claim> {
        let mut held = self.inner.held();
        if held.passes_stopped || held.holders.contains_key(&session) {
            return None;
        }
        held.holders
            .insert(session, Holder::NotePass(cancellation.clone()));
        drop(held);
        Some(self.claim(session))
    }

    /// Cancels every running note pass and starts no new one, for a Host
    /// that is stopping: each settles as cancelled before its claim goes.
    pub(crate) fn stop_passes(&self) {
        let mut held = self.inner.held();
        held.passes_stopped = true;
        for holder in held.holders.values() {
            if let Holder::NotePass(cancellation) = holder {
                cancellation.cancel();
            }
        }
    }

    /// Whether [`Self::stop_passes`] was called.
    pub(crate) fn passes_stopped(&self) -> bool {
        self.inner.held().passes_stopped
    }

    /// The instance of `session` open in this process, or the one `load`
    /// opens when none is, for a note pass, which waits while another opener
    /// loads it. An instance that is closing is not handed out: `load` opens
    /// the session again, which its storage refuses while the closing instance
    /// still holds the kernel past a brief handoff.
    ///
    /// # Errors
    ///
    /// Returns what `load` returns.
    pub(crate) async fn open(
        &self,
        session: Uuid,
        load: impl Future<Output = Result<Arc<AgentSession>, LocalHostError>>,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        let slot = self.slot(session);
        Self::open_in(slot.lock().await, load).await
    }

    /// [`Self::open`] for a surface, which waits for another opener's load,
    /// a pass's or a surface's, no longer than [`SURFACE_WAIT`]. It cancels
    /// no pass: a load does not watch cancellation, so cancelling would not
    /// shorten the wait, and a surface that only opens the session, as ACP's
    /// `load_session` does, leaves a pass to finish. Its turn's claim cancels
    /// the pass.
    ///
    /// # Errors
    ///
    /// Returns what `load` returns, or [`LocalHostError::SessionStillOpening`].
    pub(crate) async fn open_for_surface(
        &self,
        session: Uuid,
        load: impl Future<Output = Result<Arc<AgentSession>, LocalHostError>>,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        let slot = self.slot(session);
        let held = tokio::time::timeout(SURFACE_WAIT, slot.lock())
            .await
            .map_err(|_| LocalHostError::SessionStillOpening)?;
        Self::open_in(held, load).await
    }

    fn slot(&self, session: Uuid) -> Arc<tokio::sync::Mutex<Weak<AgentSession>>> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        // A slot only the map holds, whose instance is gone, is dropped.
        open.retain(|_, slot| {
            Arc::strong_count(slot) > 1 || slot.try_lock().is_ok_and(|open| open.strong_count() > 0)
        });
        Arc::clone(open.entry(session).or_default())
    }

    async fn open_in(
        mut slot: tokio::sync::MutexGuard<'_, Weak<AgentSession>>,
        load: impl Future<Output = Result<Arc<AgentSession>, LocalHostError>>,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        if let Some(open) = slot.upgrade()
            && open.accepts_work()
        {
            return Ok(open);
        }
        let opened = load.await?;
        *slot = Arc::downgrade(&opened);
        Ok(opened)
    }

    /// Forgets a deleted session's slot.
    pub(crate) fn forget(&self, session: Uuid) {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&session);
    }

    fn claim(&self, session: Uuid) -> Claim {
        Claim {
            inner: Arc::clone(&self.inner),
            session,
        }
    }
}

impl Inner {
    /// The map stays consistent under a panic: each change is one insert,
    /// update or removal.
    fn held(&self) -> MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut held = self.inner.held();
        match held.holders.get_mut(&self.session) {
            Some(Holder::Surfaces(count)) if count.get() > 1 => {
                *count = NonZeroUsize::new(count.get() - 1).unwrap_or(NonZeroUsize::MIN);
            }
            Some(_) => {
                held.holders.remove(&self.session);
            }
            None => {}
        }
        drop(held);
        self.inner.released.notify_waiters();
    }
}

#[cfg(test)]
mod tests;
