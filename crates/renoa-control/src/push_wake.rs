//! When a journalled record wakes the owner's phone, and the durable marker
//! that says a wake is owed.
//!
//! A wake is a hint outside the journal: it carries no task content, and the
//! app learns what changed by reading the journal from its own cursors. The
//! marker is written by the transaction that appends the record, so it is
//! durable before the node hears `execution_events_accepted`, it survives a
//! restart, and a retried batch that appends nothing marks nothing.

use std::sync::Arc;

use renoa_protocol::{ExecutionEventKind, ExecutionTerminal, PrincipalId};
use rusqlite::{Transaction, params};

use crate::{
    ControlError, TaskEventKind,
    control_schema::open_connection,
    store::{ControlStore, blocking, id_error, sqlite_error},
};

/// Why a record wakes the owner, or `None` when it does not.
///
/// Every variant is decided here with no wildcard, so a new record kind cannot
/// reach the owner's phone, or fail to, without a decision. A record wakes
/// when it is something the app would show unless that chat is on screen: an
/// agent's reply with text other than whitespace, or a failed run. A submitted
/// command does not (the owner or another agent wrote it), nor does a run's
/// progress, a completed run (its reply already woke), or a cancelled one (the
/// owner pressed stop).
pub(crate) fn wake_reason(kind: &TaskEventKind) -> Option<&'static str> {
    match kind {
        TaskEventKind::CommandSubmitted { .. } => None,
        TaskEventKind::ExecutionEvent { event, .. } => match &event.kind {
            ExecutionEventKind::AssistantMessage { text } if !text.trim().is_empty() => {
                Some("assistant_message")
            }
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Failed { .. },
            } => Some("execution_failed"),
            ExecutionEventKind::AssistantMessage { .. }
            | ExecutionEventKind::ExecutionStarted
            | ExecutionEventKind::TurnStarted
            | ExecutionEventKind::ToolStarted { .. }
            | ExecutionEventKind::ToolFinished { .. }
            | ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed | ExecutionTerminal::Cancelled { .. },
            } => None,
        },
    }
}

/// Marks a wake as owed to `principal_id` inside the caller's transaction:
/// creates the principal's one marker or advances its generation past the
/// settled one.
pub(crate) fn record(
    transaction: &Transaction<'_>,
    principal_id: PrincipalId,
) -> Result<(), ControlError> {
    transaction
        .execute(
            "INSERT INTO push_wakes (principal_id, generation) VALUES (?1, 1)
             ON CONFLICT (principal_id) DO UPDATE SET generation = generation + 1",
            [principal_id.to_string()],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

/// One principal's owed wake as the dispatcher read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwedWake {
    pub(crate) principal_id: PrincipalId,
    pub(crate) generation: i64,
}

impl ControlStore {
    pub(crate) async fn owed_wakes(&self) -> Result<Vec<OwedWake>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT principal_id, generation FROM push_wakes
                     WHERE generation > settled ORDER BY principal_id",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(sqlite_error)?;
            let mut wakes = Vec::new();
            for row in rows {
                let (principal_id, generation) = row.map_err(sqlite_error)?;
                wakes.push(OwedWake {
                    principal_id: PrincipalId::from_uuid(principal_id.parse().map_err(id_error)?),
                    generation,
                });
            }
            Ok(wakes)
        })
        .await
    }

    /// Settles the marker only at `wake.generation`, so a wake recorded since
    /// that generation was sent stays owed. The row stays, so the next wake
    /// takes a later generation. Returns whether it settled one.
    pub(crate) async fn settle_wake(&self, wake: OwedWake) -> Result<bool, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            open_connection(&path)?
                .execute(
                    "UPDATE push_wakes SET settled = generation
                     WHERE principal_id = ?1 AND generation = ?2 AND settled < generation",
                    params![wake.principal_id.to_string(), wake.generation],
                )
                .map(|settled| settled > 0)
                .map_err(sqlite_error)
        })
        .await
    }
}

#[cfg(test)]
mod tests;
