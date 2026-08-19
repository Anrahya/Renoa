use std::{
    collections::HashSet,
    panic::resume_unwind,
    sync::{Arc, Mutex},
};

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use crate::{
    EffectAdapter, EffectCompletion, EffectInvocation, KernelError, SessionId, StoreError,
    StoreErrorKind, database::DatabaseLease,
};

pub(crate) struct SessionDriveLease {
    ownership: Arc<SessionDriveOwnership>,
}

impl SessionDriveLease {
    pub(crate) fn acquire(
        running: &Arc<Mutex<HashSet<SessionId>>>,
        database: &Arc<DatabaseLease>,
        session_id: SessionId,
    ) -> Result<Self, KernelError> {
        let mut sessions = running.lock().map_err(|error| {
            KernelError::Store(StoreError::message(
                StoreErrorKind::Ownership,
                format!("session ownership lock was poisoned: {error}"),
            ))
        })?;
        if !sessions.insert(session_id) {
            return Err(KernelError::Busy(session_id));
        }
        Ok(Self {
            ownership: Arc::new(SessionDriveOwnership {
                running: Arc::clone(running),
                _database: Arc::clone(database),
                session_id,
            }),
        })
    }
}

impl Clone for SessionDriveLease {
    fn clone(&self) -> Self {
        Self {
            ownership: Arc::clone(&self.ownership),
        }
    }
}

struct SessionDriveOwnership {
    running: Arc<Mutex<HashSet<SessionId>>>,
    // Keeps the OS writer lock alive when cleanup outlives the Kernel handle.
    _database: Arc<DatabaseLease>,
    session_id: SessionId,
}

impl Drop for SessionDriveOwnership {
    fn drop(&mut self) {
        if let Ok(mut running) = self.running.lock() {
            running.remove(&self.session_id);
        }
    }
}

struct CancelEffectOnDrop(Option<CancellationToken>);

impl CancelEffectOnDrop {
    const fn new(cancellation: CancellationToken) -> Self {
        Self(Some(cancellation))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for CancelEffectOnDrop {
    fn drop(&mut self) {
        if let Some(cancellation) = self.0.take() {
            cancellation.cancel();
        }
    }
}

pub(crate) async fn supervise_effect(
    executor: &Handle,
    adapter: Arc<dyn EffectAdapter>,
    invocation: EffectInvocation,
    lease: SessionDriveLease,
) -> Result<EffectCompletion, KernelError> {
    let mut cancel_on_drop = CancelEffectOnDrop::new(invocation.cancellation.clone());
    let task = executor.spawn(async move {
        let _effect_lease = lease;
        adapter.invoke(invocation).await
    });
    let completion = match task.await {
        Ok(completion) => completion,
        Err(error) if error.is_panic() => resume_unwind(error.into_panic()),
        Err(error) => return Err(KernelError::EffectTask(error)),
    };
    cancel_on_drop.disarm();
    Ok(completion)
}
