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
//! it, at most [`SURFACE_WAIT`], then fails without running anything. A pass
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
pub(crate) const SURFACE_WAIT: Duration = Duration::from_secs(20);

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
    /// [`SURFACE_WAIT`].
    pub(crate) async fn surface(&self, session: Uuid) -> Result<Claim, LocalHostError> {
        tokio::time::timeout(SURFACE_WAIT, self.wait_for(session))
            .await
            .map_err(|_| {
                LocalHostError::InvalidRequest(
                    "a memory pass in this conversation did not stop in time; nothing was run, so send it again"
                        .to_owned(),
                )
            })
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
    /// opens when none is. An instance that is closing is not handed out:
    /// `load` opens the session again, which its storage refuses while the
    /// closing instance still holds the kernel past a brief handoff.
    ///
    /// # Errors
    ///
    /// Returns what `load` returns.
    pub(crate) async fn open(
        &self,
        session: Uuid,
        load: impl Future<Output = Result<Arc<AgentSession>, LocalHostError>>,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        let slot = {
            let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
            // A slot only the map holds, whose instance is gone, is dropped.
            open.retain(|_, slot| {
                Arc::strong_count(slot) > 1
                    || slot.try_lock().is_ok_and(|open| open.strong_count() > 0)
            });
            Arc::clone(open.entry(session).or_default())
        };
        let mut slot = slot.lock().await;
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
mod tests {
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::{Claims, SURFACE_WAIT};

    #[tokio::test]
    async fn a_surface_cancels_the_pass_and_waits_for_it_and_a_pass_never_takes_a_held_session() {
        let claims = Claims::default();
        let session = Uuid::new_v4();
        let pass = CancellationToken::new();
        let held = claims.note_pass(session, &pass).expect("a free session");
        assert!(
            claims
                .note_pass(session, &CancellationToken::new())
                .is_none(),
            "one pass at a time"
        );
        let surface = claims.surface(session);
        tokio::pin!(surface);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), surface.as_mut())
                .await
                .is_err(),
            "the surface waits while the pass holds the session"
        );
        assert!(pass.is_cancelled(), "and has cancelled it");
        drop(held);
        let first = surface.await.expect("the pass let go");
        let second = claims.surface(session).await.expect("surfaces share");
        assert!(
            claims
                .note_pass(session, &CancellationToken::new())
                .is_none(),
            "a pass never takes a session a surface holds"
        );
        drop(first);
        assert!(
            claims
                .note_pass(session, &CancellationToken::new())
                .is_none()
        );
        drop(second);
        assert!(
            claims
                .note_pass(session, &CancellationToken::new())
                .is_some(),
            "free once every surface let go"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_surface_gives_up_on_a_pass_that_does_not_let_go_and_claims_nothing() {
        let claims = Claims::default();
        let session = Uuid::new_v4();
        let pass = CancellationToken::new();
        let held = claims.note_pass(session, &pass).expect("a free session");
        let started = tokio::time::Instant::now();
        let refused = tokio::time::timeout(SURFACE_WAIT * 2, claims.surface(session))
            .await
            .expect("the surface gives up by itself");
        assert!(pass.is_cancelled());
        let error = refused.err().expect("the surface gives up");
        assert!(error.to_string().contains("send it again"), "{error}");
        assert_eq!(started.elapsed(), SURFACE_WAIT);
        drop(held);
        assert!(
            claims
                .note_pass(session, &CancellationToken::new())
                .is_some(),
            "the refused surface holds nothing"
        );
    }

    #[tokio::test]
    async fn stopping_passes_cancels_the_running_one_and_refuses_the_next() {
        let claims = Claims::default();
        let (running, idle) = (Uuid::new_v4(), Uuid::new_v4());
        let pass = CancellationToken::new();
        let held = claims.note_pass(running, &pass).expect("a free session");
        claims.stop_passes();
        assert!(pass.is_cancelled());
        assert!(claims.passes_stopped());
        assert!(
            claims.note_pass(idle, &CancellationToken::new()).is_none(),
            "no pass starts once stopped"
        );
        drop(held);
        assert!(claims.surface(running).await.is_ok(), "surfaces go on");
    }
}
