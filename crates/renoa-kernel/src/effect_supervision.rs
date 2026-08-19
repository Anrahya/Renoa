use std::{
    collections::HashMap,
    panic::resume_unwind,
    sync::{Arc, Mutex},
};

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use crate::{
    EffectAdapter, EffectCompletion, EffectInvocation, KernelError, OperationId, SessionId,
    StoreError, StoreErrorKind, database::DatabaseLease,
};

pub(crate) type RunningSessions = Arc<Mutex<HashMap<SessionId, RunningSession>>>;

pub(crate) struct RunningSession {
    operation_id: Option<OperationId>,
    cancellation: CancellationToken,
}

pub(crate) struct SessionDriveLease {
    ownership: Arc<SessionDriveOwnership>,
}

impl SessionDriveLease {
    pub(crate) fn acquire(
        running: &RunningSessions,
        database: &Arc<DatabaseLease>,
        session_id: SessionId,
    ) -> Result<Self, KernelError> {
        let mut sessions = running.lock().map_err(|error| {
            KernelError::Store(StoreError::message(
                StoreErrorKind::Ownership,
                format!("session ownership lock was poisoned: {error}"),
            ))
        })?;
        if sessions.contains_key(&session_id) {
            return Err(KernelError::Busy(session_id));
        }
        let cancellation = CancellationToken::new();
        sessions.insert(
            session_id,
            RunningSession {
                operation_id: None,
                cancellation: cancellation.clone(),
            },
        );
        Ok(Self {
            ownership: Arc::new(SessionDriveOwnership {
                running: Arc::clone(running),
                _database: Arc::clone(database),
                session_id,
                cancellation,
            }),
        })
    }

    pub(crate) fn bind(&self, operation_id: OperationId) -> Result<(), KernelError> {
        let mut sessions = self.ownership.running.lock().map_err(|error| {
            KernelError::Store(StoreError::message(
                StoreErrorKind::Ownership,
                format!("session ownership lock was poisoned: {error}"),
            ))
        })?;
        let running = sessions
            .get_mut(&self.ownership.session_id)
            .ok_or_else(|| {
                KernelError::Store(StoreError::message(
                    StoreErrorKind::Ownership,
                    "session ownership disappeared before operation binding",
                ))
            })?;
        match running.operation_id {
            None => running.operation_id = Some(operation_id),
            Some(bound) if bound == operation_id => {}
            Some(bound) => {
                return Err(KernelError::Store(StoreError::message(
                    StoreErrorKind::Ownership,
                    format!("session drive changed operation from {bound} to {operation_id}"),
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn effect_cancellation(&self) -> CancellationToken {
        self.ownership.cancellation.child_token()
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
    running: RunningSessions,
    // Keeps the OS writer lock alive when cleanup outlives the Kernel handle.
    _database: Arc<DatabaseLease>,
    session_id: SessionId,
    cancellation: CancellationToken,
}

pub(crate) fn signal_running_operation(
    running: &RunningSessions,
    session_id: SessionId,
    operation_id: OperationId,
) -> Result<(), KernelError> {
    let cancellation = running
        .lock()
        .map_err(|error| {
            KernelError::Store(StoreError::message(
                StoreErrorKind::Ownership,
                format!("session ownership lock was poisoned: {error}"),
            ))
        })?
        .get(&session_id)
        .filter(|entry| entry.operation_id == Some(operation_id))
        .map(|entry| entry.cancellation.clone());
    if let Some(cancellation) = cancellation {
        cancellation.cancel();
    }
    Ok(())
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
