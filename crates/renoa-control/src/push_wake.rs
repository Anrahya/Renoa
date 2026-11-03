//! When a journalled record wakes the owner's phone, and the durable marker
//! that says a wake is owed.
//!
//! A wake is a hint outside the journal: it carries no task content, and the
//! app learns what changed by reading the journal from its own cursors. The
//! marker is written by the transaction that appends the record, so it is
//! durable before the node hears `execution_events_accepted`, it survives a
//! restart, and a retried batch that appends nothing marks nothing. The
//! records of a command submitted quiet, such as a heartbeat's run, mark no
//! wake: what it has for the owner it asks for with `request_wake`.
//!
//! A node also asks for a wake, with `request_wake`, for something it has for
//! its owner that the journal does not hold. The request carries an id and an
//! urgency and nothing else; the principal is the node's owner. A `now`
//! request makes the wake ring: the marker remembers the generation it rang
//! at, so the ring stays owed until every device had one, whatever wakes
//! follow.

use std::{sync::Arc, time::SystemTime};

use renoa_protocol::{CommandEnvelope, ExecutionEventKind, ExecutionTerminal, PrincipalId};
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    ControlError, NodeId, ServerMessage, TaskEventKind, control_log,
    control_schema::open_connection,
    coordinator::CoordinatorState,
    identity_store::timestamp_millis,
    node_ownership::node_owner,
    store::{ControlStore, blocking, id_error, json_error, sqlite_error},
    wire::send_control_error,
};

/// How loudly a node asks to wake its owner. There is no quiet value: what
/// can wait until the owner opens the app asks for no wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeUrgency {
    /// A wake like a reply's.
    Soon,
    /// A wake that rings.
    Now,
}

impl WakeUrgency {
    const fn reason(self) -> &'static str {
        match self {
            Self::Soon => "requested_soon",
            Self::Now => "requested_now",
        }
    }
}

/// How long a requested wake id is remembered. A repeat within it records
/// nothing; a later one is one more wake, which costs the owner a hint.
const REQUEST_MEMORY_MS: i64 = 24 * 3_600_000;

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

/// Whether `kind` is a record of a command submitted quiet, or of its
/// execution.
pub(crate) fn of_quiet_command(
    transaction: &Transaction<'_>,
    kind: &TaskEventKind,
) -> Result<bool, ControlError> {
    let command_id = match kind {
        TaskEventKind::ExecutionEvent { command_id, .. } => command_id,
        TaskEventKind::CommandSubmitted { command } => return Ok(command.input.quiet()),
    };
    let Some(command_json) = transaction
        .query_row(
            "SELECT command_json FROM commands WHERE command_id = ?1",
            [command_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
    else {
        return Ok(false);
    };
    let command: CommandEnvelope = serde_json::from_str(&command_json).map_err(json_error)?;
    Ok(command.input.quiet())
}

/// Marks a wake as owed to `principal_id` inside the caller's transaction:
/// creates the principal's one marker or advances its generation past the
/// settled one. A wake that `rings` records its generation as the ring's.
pub(crate) fn record(
    transaction: &Transaction<'_>,
    principal_id: PrincipalId,
    rings: bool,
) -> Result<(), ControlError> {
    transaction
        .execute(
            "INSERT INTO push_wakes (principal_id, generation, ring_generation)
             VALUES (?1, 1, CASE WHEN ?2 THEN 1 ELSE 0 END)
             ON CONFLICT (principal_id) DO UPDATE SET
                generation = generation + 1,
                ring_generation = CASE WHEN ?2 THEN generation + 1 ELSE ring_generation END",
            params![principal_id.to_string(), rings],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

/// Answers a node's `request_wake`: records the wake for the node's owner
/// unless this id was asked for within [`REQUEST_MEMORY_MS`], and
/// acknowledges once that is durable. A node without an owner is refused.
pub(crate) async fn request(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    node_id: NodeId,
    request_id: u64,
    wake_id: Uuid,
    urgency: WakeUrgency,
) {
    let recorded = match timestamp_millis(SystemTime::now()) {
        Ok(now_ms) => {
            state
                .store
                .request_wake(node_id, wake_id, urgency, now_ms)
                .await
        }
        Err(error) => Err(error),
    };
    match recorded {
        Ok(recorded) => {
            if let Some(principal_id) = recorded {
                control_log::event(
                    "info",
                    "push_wake_recorded",
                    &serde_json::json!({
                        "principal_id": principal_id,
                        "node_id": node_id,
                        "wake_id": wake_id,
                        "reason": urgency.reason(),
                    }),
                );
                state.push_wakes.notify_one();
            }
            let _ = outgoing
                .send(ServerMessage::WakeRequested { request_id })
                .await;
        }
        Err(error) => send_control_error(outgoing, Some(request_id), &error).await,
    }
}

/// When a principal's wake row is owed. The one place the predicate is
/// written; the tests outside the crate copy it, and
/// `tests::every_test_counts_owed_wakes_with_the_one_predicate` checks them.
pub(crate) const OWED: &str = "generation > settled";
/// When an owed wake rings.
const RINGS: &str = "ring_generation > settled";

/// One principal's owed wake as the dispatcher read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwedWake {
    pub(crate) principal_id: PrincipalId,
    pub(crate) generation: i64,
    /// The generation a ring was asked for at, while one is owed.
    pub(crate) ring: Option<i64>,
}

impl ControlStore {
    /// Records a wake `wake_id` asks of `node_id`'s owner, and returns that
    /// owner, or `None` when the id was asked for already. Repeats older than
    /// [`REQUEST_MEMORY_MS`] are forgotten in the same transaction.
    async fn request_wake(
        &self,
        node_id: NodeId,
        wake_id: Uuid,
        urgency: WakeUrgency,
        now_ms: i64,
    ) -> Result<Option<PrincipalId>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let principal_id = node_owner(&transaction, node_id)?.ok_or_else(|| {
                ControlError::role(format!(
                    "node {node_id} has no owner, so it has no one to wake"
                ))
            })?;
            transaction
                .execute(
                    "DELETE FROM push_wake_requests WHERE received_at_ms < ?1",
                    [now_ms.saturating_sub(REQUEST_MEMORY_MS)],
                )
                .map_err(sqlite_error)?;
            let first = transaction
                .execute(
                    "INSERT OR IGNORE INTO push_wake_requests (principal_id, wake_id, received_at_ms)
                     VALUES (?1, ?2, ?3)",
                    params![principal_id.to_string(), wake_id.to_string(), now_ms],
                )
                .map_err(sqlite_error)?
                > 0;
            if first {
                record(&transaction, principal_id, urgency == WakeUrgency::Now)?;
            }
            transaction.commit().map_err(sqlite_error)?;
            Ok(first.then_some(principal_id))
        })
        .await
    }

    pub(crate) async fn owed_wakes(&self) -> Result<Vec<OwedWake>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(&format!(
                    "SELECT principal_id, generation,
                        CASE WHEN {RINGS} THEN ring_generation END
                     FROM push_wakes WHERE {OWED} ORDER BY principal_id"
                ))
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                })
                .map_err(sqlite_error)?;
            let mut wakes = Vec::new();
            for row in rows {
                let (principal_id, generation, ring) = row.map_err(sqlite_error)?;
                wakes.push(OwedWake {
                    principal_id: PrincipalId::from_uuid(principal_id.parse().map_err(id_error)?),
                    generation,
                    ring,
                });
            }
            Ok(wakes)
        })
        .await
    }

    /// Settles the marker only at `generation`, so a wake recorded since that
    /// generation was sent stays owed, and a ring asked for since stays owed
    /// too. The row stays, so the next wake takes a later generation. Returns
    /// whether it settled one.
    pub(crate) async fn settle_wake(
        &self,
        principal_id: PrincipalId,
        generation: i64,
    ) -> Result<bool, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            open_connection(&path)?
                .execute(
                    &format!(
                        "UPDATE push_wakes SET settled = generation
                         WHERE principal_id = ?1 AND generation = ?2 AND {OWED}"
                    ),
                    params![principal_id.to_string(), generation],
                )
                .map(|settled| settled > 0)
                .map_err(sqlite_error)
        })
        .await
    }
}

#[cfg(test)]
mod tests;
