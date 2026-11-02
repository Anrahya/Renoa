//! Who holds an Agent session in this process: its surfaces, any number at a
//! time, or one `renoa.memory` note pass.
//!
//! The kernel lets one handle open a session at a time. A surface that opens a
//! session a pass holds cancels the pass and waits until it lets go, so a
//! person's message through this process is not refused because a pass was
//! reviewing their conversation; a pass never takes a session a surface of
//! this process holds. Claims do not reach another process: there the
//! kernel's own lock refuses the second opener. Every
//! [`AgentSession`](super::AgentSession) owns the claim it was opened under and
//! releases it after its kernel handle, so a waiting surface opens a closed
//! session.

use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The claims of one Host's sessions.
#[derive(Default)]
pub(crate) struct Claims {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    held: Mutex<HashMap<Uuid, Holder>>,
    /// Signalled whenever a claim is released.
    released: Notify,
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
    /// Claims `session` for a surface, cancelling a note pass that holds it and
    /// waiting until that pass lets go.
    pub(crate) async fn surface(&self, session: Uuid) -> Claim {
        loop {
            let released = self.inner.released.notified();
            tokio::pin!(released);
            // Registered before the map is read, so a release between the
            // read and the wait still wakes this surface.
            released.as_mut().enable();
            {
                let mut held = self.inner.held();
                match held.get_mut(&session) {
                    None => {
                        held.insert(session, Holder::Surfaces(NonZeroUsize::MIN));
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
    /// anyone holds it.
    pub(crate) fn note_pass(
        &self,
        session: Uuid,
        cancellation: &CancellationToken,
    ) -> Option<Claim> {
        let mut held = self.inner.held();
        if held.contains_key(&session) {
            return None;
        }
        held.insert(session, Holder::NotePass(cancellation.clone()));
        drop(held);
        Some(self.claim(session))
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
    fn held(&self) -> MutexGuard<'_, HashMap<Uuid, Holder>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut held = self.inner.held();
        match held.get_mut(&self.session) {
            Some(Holder::Surfaces(count)) if count.get() > 1 => {
                *count = NonZeroUsize::new(count.get() - 1).unwrap_or(NonZeroUsize::MIN);
            }
            Some(_) => {
                held.remove(&self.session);
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

    use super::Claims;

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
        let first = surface.await;
        let second = claims.surface(session).await;
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
}
