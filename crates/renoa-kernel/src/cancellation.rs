use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::Value;

use crate::{
    AgentId, CancellationId, Checkpoint, Command, EffectId, EventCursor, Kernel, KernelError,
    NewEvent, OperationId, OperationOutcome, Runtime, RuntimeManifest, SemanticEvent, SessionId,
    SettledEffect,
    admission::{from_sql_integer, parse_agent_id, parse_operation_id},
    decision_store::append_events,
    effect_store::{mark_outcome_unknown, parse_effect_id},
    effect_supervision::signal_running_operation,
    events::load_event_page,
    inspection::require_state_version,
    operation_phase::OperationPhase,
    runtime::require_compatible_checkpoint,
    schema::{json_error, sqlite_error},
};

mod store;

use store::{decode_command, decode_manifest, load_cancellation_effect, load_operation};

/// Exact persisted effect identity without a definite outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsettledEffect {
    pub effect_id: EffectId,
    pub binding: String,
    pub binding_revision: String,
    pub request: Value,
}

/// The durable external-effect fact visible while closing cancellation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CancellationEffect {
    /// Durable intent exists and dispatch provably never started.
    NotDispatched(UnsettledEffect),
    /// The adapter produced one exact definite result.
    Settled(SettledEffect),
    /// Dispatch may have happened but no definite result is known.
    OutcomeUnknown(UnsettledEffect),
}

/// Owned durable input for loop-defined cancellation closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancellationInput {
    pub agent_id: AgentId,
    pub session_id: SessionId,
    pub operation_id: OperationId,
    pub command: Command,
    pub events: Vec<SemanticEvent>,
    pub checkpoint: Option<Checkpoint>,
    pub effect: Option<CancellationEffect>,
}

/// The loop-owned state and semantic events that close cancellation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancellationTransition {
    pub checkpoint: Checkpoint,
    pub events: Vec<NewEvent>,
}

struct PendingCancellation {
    input: CancellationInput,
    manifest: RuntimeManifest,
    transition_version: i64,
    event_high_water: u64,
}

impl Kernel {
    /// Durably requests cancellation of one exact active operation.
    ///
    /// The request is committed before a process-local signal is delivered.
    /// Repeating the same identity and target is idempotent, including after
    /// cancellation has settled.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::CancellationConflict`] when the identity is bound
    /// to another target, or [`KernelError::OperationNotCancellable`] when the
    /// supplied operation is not the session's active operation.
    pub fn request_cancellation(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
        cancellation_id: CancellationId,
    ) -> Result<(), KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let session_key = session_id.to_string();
        let operation_key = operation_id.to_string();
        if let Some((stored_session, stored_operation)) = transaction
            .query_row(
                "SELECT session_id, operation_id FROM cancellation_requests
                 WHERE cancellation_id = ?1",
                [cancellation_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(sqlite_error)?
        {
            if stored_session != session_key || stored_operation != operation_key {
                return Err(KernelError::CancellationConflict {
                    cancellation_id,
                    operation_id: parse_operation_id(&stored_operation)?,
                });
            }
            let should_signal = transaction
                .query_row(
                    "SELECT COALESCE(active_operation_id = ?2, FALSE)
                     FROM sessions WHERE session_id = ?1",
                    params![session_key, operation_key],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => {
                        KernelError::SessionNotFound(session_id)
                    }
                    other => sqlite_error(other),
                })?;
            transaction.commit().map_err(sqlite_error)?;
            if should_signal {
                signal_running_operation(&self.running_sessions, session_id, operation_id)?;
            }
            return Ok(());
        }

        let active_phase = transaction
            .query_row(
                "SELECT o.phase, s.active_operation_id
                 FROM operations AS o
                 JOIN sessions AS s ON s.session_id = o.session_id
                 WHERE o.session_id = ?1 AND o.operation_id = ?2",
                params![session_key, operation_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(sqlite_error)?;
        let Some((phase, active_operation)) = active_phase else {
            return Err(KernelError::OperationNotCancellable(operation_id));
        };
        if active_operation.as_deref() != Some(operation_key.as_str())
            || !OperationPhase::from_database(&phase)?.is_cancellable()
        {
            return Err(KernelError::OperationNotCancellable(operation_id));
        }
        transaction
            .execute(
                "INSERT INTO cancellation_requests (
                    cancellation_id, session_id, operation_id
                 ) VALUES (?1, ?2, ?3)",
                params![cancellation_id.to_string(), session_key, operation_key],
            )
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
        signal_running_operation(&self.running_sessions, session_id, operation_id)
    }

    pub(crate) fn close_requested_cancellation(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
        runtime: &Runtime,
    ) -> Result<Option<OperationOutcome>, KernelError> {
        let Some(pending) = self.load_requested_cancellation(session_id, operation_id)? else {
            return Ok(None);
        };
        if &pending.manifest != runtime.manifest() {
            return Err(KernelError::RuntimeMismatch);
        }
        let transition = runtime
            .plugin
            .cancel_operation(pending.input)
            .map_err(KernelError::Loop)?;
        require_compatible_checkpoint(&pending.manifest, Some(&transition.checkpoint))?;
        validate_transition(&transition)?;
        self.commit_cancellation(
            session_id,
            operation_id,
            pending.transition_version,
            pending.event_high_water,
            &transition,
        )?;
        #[cfg(test)]
        self.crash_if(crate::CrashPoint::CancellationCommitted);
        Ok(Some(OperationOutcome::Cancelled))
    }

    fn load_requested_cancellation(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Result<Option<PendingCancellation>, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        if !cancellation_requested(&transaction, operation_id)? {
            transaction.commit().map_err(sqlite_error)?;
            return Ok(None);
        }
        let (agent_id, active_operation) = transaction
            .query_row(
                "SELECT agent_id, active_operation_id FROM sessions
                 WHERE session_id = ?1",
                [session_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or(KernelError::SessionNotFound(session_id))?;
        let operation_key = operation_id.to_string();
        if active_operation.as_deref() != Some(operation_key.as_str()) {
            return Err(KernelError::Corrupt(
                "cancelled operation no longer owns its session".to_owned(),
            ));
        }
        let mut stored = load_operation(&transaction, session_id, operation_id)?;
        require_state_version(stored.state_version)?;
        let mut phase = OperationPhase::from_database(&stored.phase)?;
        if !phase.is_cancellable() || stored.outcome_json.is_some() {
            return Err(KernelError::Corrupt(
                "cancellation request targets a non-cancellable operation".to_owned(),
            ));
        }
        if phase == OperationPhase::EffectDispatched {
            let effect_id = stored.current_effect_id.as_deref().ok_or_else(|| {
                KernelError::Corrupt("dispatched operation has no current effect".to_owned())
            })?;
            mark_outcome_unknown(
                &transaction,
                operation_id,
                parse_effect_id(effect_id)?,
                stored.transition_version,
            )?;
            stored.transition_version = stored
                .transition_version
                .checked_add(1)
                .ok_or_else(|| KernelError::Corrupt("transition version overflowed".to_owned()))?;
            phase = OperationPhase::OutcomeUnknown;
        }
        let manifest = decode_manifest(stored.manifest_json)?;
        let checkpoint = stored
            .checkpoint_json
            .map(|value| serde_json::from_str(&value).map_err(json_error))
            .transpose()?;
        require_compatible_checkpoint(&manifest, checkpoint.as_ref())?;
        let command = decode_command(&stored.command_id, &stored.command_json)?;
        let effect = load_cancellation_effect(
            &transaction,
            operation_id,
            phase,
            stored.current_effect_id.as_deref(),
            stored.input_effect_id.as_deref(),
            &manifest,
        )?;
        let page = load_event_page(&transaction, session_id, EventCursor::START)?;
        let pending = PendingCancellation {
            input: CancellationInput {
                agent_id: parse_agent_id(&agent_id)?,
                session_id,
                operation_id,
                command,
                events: page.events,
                checkpoint,
                effect,
            },
            manifest,
            transition_version: stored.transition_version,
            event_high_water: page.next_cursor.next_sequence(),
        };
        transaction.commit().map_err(sqlite_error)?;
        Ok(Some(pending))
    }

    fn commit_cancellation(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
        expected_transition: i64,
        expected_event_high_water: u64,
        transition: &CancellationTransition,
    ) -> Result<(), KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let event_high_water = transaction
            .query_row(
                "SELECT next_event_sequence FROM sessions
                 WHERE session_id = ?1 AND active_operation_id = ?2",
                params![session_id.to_string(), operation_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or_else(|| KernelError::Corrupt("cancelled operation lost ownership".to_owned()))?;
        if from_sql_integer(event_high_water, "event high-water mark")? != expected_event_high_water
        {
            return Err(KernelError::Corrupt(
                "semantic history changed during cancellation".to_owned(),
            ));
        }
        if !cancellation_requested(&transaction, operation_id)? {
            return Err(KernelError::Corrupt(
                "cancellation request disappeared before settlement".to_owned(),
            ));
        }
        let phase = transaction
            .query_row(
                "SELECT phase FROM operations
                 WHERE session_id = ?1 AND operation_id = ?2
                   AND transition_version = ?3 AND outcome_json IS NULL",
                params![
                    session_id.to_string(),
                    operation_id.to_string(),
                    expected_transition,
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or_else(|| KernelError::Corrupt("cancelled operation changed".to_owned()))?;
        if !OperationPhase::from_database(&phase)?.is_cancellable() {
            return Err(KernelError::Corrupt(
                "cancelled operation reached a terminal phase".to_owned(),
            ));
        }
        append_events(&transaction, session_id, operation_id, &transition.events)?;
        let outcome = OperationOutcome::Cancelled;
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET phase = 'cancelled', checkpoint_json = ?4,
                     current_effect_id = NULL, input_effect_id = NULL,
                     outcome_json = ?5, transition_version = transition_version + 1
                 WHERE session_id = ?1 AND operation_id = ?2
                   AND transition_version = ?3 AND phase = ?6",
                params![
                    session_id.to_string(),
                    operation_id.to_string(),
                    expected_transition,
                    serde_json::to_string(&transition.checkpoint).map_err(json_error)?,
                    serde_json::to_string(&outcome).map_err(json_error)?,
                    phase,
                ],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "cancellation compare-and-set failed".to_owned(),
            ));
        }
        let changed = transaction
            .execute(
                "UPDATE sessions SET active_operation_id = NULL
                 WHERE session_id = ?1 AND active_operation_id = ?2",
                params![session_id.to_string(), operation_id.to_string()],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "cancelled operation did not own its session".to_owned(),
            ));
        }
        transaction.commit().map_err(sqlite_error)
    }
}

pub(crate) fn cancellation_requested(
    connection: &rusqlite::Connection,
    operation_id: OperationId,
) -> Result<bool, KernelError> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM cancellation_requests WHERE operation_id = ?1
             )",
            [operation_id.to_string()],
            |row| row.get(0),
        )
        .map_err(sqlite_error)
}

fn validate_transition(transition: &CancellationTransition) -> Result<(), KernelError> {
    if transition.events.iter().any(|event| event.kind.is_empty()) {
        Err(KernelError::InvalidDecision(
            "semantic event kind cannot be empty".to_owned(),
        ))
    } else {
        Ok(())
    }
}
