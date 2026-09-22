use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    AgentId, EffectFact, EventCursor, Kernel, KernelError, OperationId, OperationOutcome, Runtime,
    RuntimeManifest, SessionId, UnknownEffectAbandonment, UnknownEffectInput,
    admission::{from_sql_integer, parse_agent_id, parse_operation_id},
    cancellation::cancellation_requested,
    decision_store::append_events,
    effect_store::{load_effect_batch_facts, parse_effect_batch_id},
    effect_supervision::SessionDriveLease,
    events::{load_event_page, validate_new_events},
    operation_phase::OperationPhase,
    operation_store::{StoredOperation, load_operation},
    runtime::require_compatible_checkpoint,
    schema::{json_error, sqlite_error},
};

const ABANDONED_REASON: &str = "effect outcome is unknown; operation was abandoned";

struct PendingAbandonment {
    input: UnknownEffectInput,
    manifest: RuntimeManifest,
    transition_version: i64,
    event_high_water: u64,
}

enum UnknownEffectState {
    Pending(Box<PendingAbandonment>),
    AlreadyAbandoned {
        manifest: RuntimeManifest,
        outcome: OperationOutcome,
    },
}

impl Kernel {
    /// Explicitly closes an operation whose effect batch has an unknown child.
    ///
    /// The kernel validates the exact active operation, frozen runtime, gapless
    /// semantic history, checkpoint, batch identity, and ordered child facts
    /// before asking the loop to close its own state. Unknown children are never
    /// invoked or rewritten as definite outcomes, and settled siblings remain
    /// unchanged.
    ///
    /// Repeating this call after a committed abandonment returns the same
    /// terminal outcome without appending duplicate events.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::NoUnknownEffect`] when the operation has no
    /// unknown batch to abandon. All compatibility, ownership, corruption,
    /// loop, and storage failures leave the operation blocked.
    pub fn abandon_unknown_effect(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
        runtime: &Runtime,
    ) -> Result<OperationOutcome, KernelError> {
        let _lease =
            SessionDriveLease::acquire(&self.running_sessions, &self.database, session_id)?;
        match self.load_unknown_effect_state(session_id, operation_id)? {
            UnknownEffectState::AlreadyAbandoned { manifest, outcome } => {
                require_runtime(&manifest, runtime)?;
                Ok(outcome)
            }
            UnknownEffectState::Pending(pending) => {
                let PendingAbandonment {
                    input,
                    manifest,
                    transition_version,
                    event_high_water,
                } = *pending;
                require_runtime(&manifest, runtime)?;
                let abandonment = runtime
                    .plugin
                    .abandon_unknown_effect(input)
                    .map_err(KernelError::Loop)?;
                require_compatible_checkpoint(&manifest, Some(&abandonment.checkpoint))?;
                validate_new_events(&abandonment.events)?;
                let outcome = self.commit_unknown_effect_abandonment(
                    session_id,
                    operation_id,
                    transition_version,
                    event_high_water,
                    &abandonment,
                )?;
                #[cfg(test)]
                self.crash_if(crate::CrashPoint::UnknownEffectAbandonmentCommitted);
                Ok(outcome)
            }
        }
    }

    fn load_unknown_effect_state(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Result<UnknownEffectState, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(sqlite_error)?;
        let (agent_id, active_operation_id) = transaction
            .query_row(
                "SELECT agent_id, active_operation_id FROM sessions WHERE session_id = ?1",
                [session_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or(KernelError::SessionNotFound(session_id))?;
        let stored = load_operation(&transaction, session_id, operation_id)?
            .ok_or(KernelError::NoUnknownEffect(operation_id))?;
        let phase = stored.phase;
        let state = match phase {
            OperationPhase::OutcomeUnknown => load_pending_abandonment(
                &transaction,
                parse_agent_id(&agent_id)?,
                session_id,
                operation_id,
                active_operation_id.as_deref(),
                stored,
            )?,
            OperationPhase::Failed => load_prior_abandonment(
                &transaction,
                session_id,
                operation_id,
                active_operation_id.as_deref(),
                stored,
            )?,
            OperationPhase::Queued
            | OperationPhase::NeedDecision
            | OperationPhase::EffectIntent
            | OperationPhase::EffectDispatched
            | OperationPhase::Waiting
            | OperationPhase::Completed => {
                return Err(KernelError::NoUnknownEffect(operation_id));
            }
            OperationPhase::Cancelled => return Err(KernelError::NoUnknownEffect(operation_id)),
        };
        transaction.commit().map_err(sqlite_error)?;
        Ok(state)
    }

    fn commit_unknown_effect_abandonment(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
        expected_transition: i64,
        expected_event_high_water: u64,
        abandonment: &UnknownEffectAbandonment,
    ) -> Result<OperationOutcome, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        if cancellation_requested(&transaction, operation_id)? {
            return Err(KernelError::CancellationPending(operation_id));
        }
        let event_high_water = transaction
            .query_row(
                "SELECT next_event_sequence FROM sessions
                 WHERE session_id = ?1 AND active_operation_id = ?2",
                params![session_id.to_string(), operation_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or_else(|| {
                KernelError::Corrupt(
                    "unknown-effect operation no longer owns its session".to_owned(),
                )
            })?;
        let event_high_water = from_sql_integer(event_high_water, "event high-water mark")?;
        if event_high_water != expected_event_high_water {
            return Err(KernelError::Corrupt(
                "semantic history changed during unknown-effect abandonment".to_owned(),
            ));
        }
        require_unknown_batch_unchanged(
            &transaction,
            session_id,
            operation_id,
            expected_transition,
        )?;

        append_events(&transaction, session_id, operation_id, &abandonment.events)?;
        let outcome = abandoned_outcome();
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET phase = 'failed', checkpoint_json = ?4,
                     current_effect_batch_id = NULL, input_effect_batch_id = NULL,
                     outcome_json = ?5, transition_version = transition_version + 1
                 WHERE session_id = ?1 AND operation_id = ?2
                   AND phase = 'outcome_unknown' AND transition_version = ?3",
                params![
                    session_id.to_string(),
                    operation_id.to_string(),
                    expected_transition,
                    serde_json::to_string(&abandonment.checkpoint).map_err(json_error)?,
                    serde_json::to_string(&outcome).map_err(json_error)?,
                ],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "unknown-effect abandonment compare-and-set failed".to_owned(),
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
                "abandoned operation did not own its session".to_owned(),
            ));
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(outcome)
    }
}

fn require_unknown_batch_unchanged(
    transaction: &rusqlite::Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
    expected_transition: i64,
) -> Result<(), KernelError> {
    let batch_is_unknown = transaction
        .query_row(
            "SELECT 1
             FROM operations AS o
             JOIN effect_batches AS b
               ON b.operation_id = o.operation_id
              AND b.batch_id = o.current_effect_batch_id
             WHERE o.session_id = ?1 AND o.operation_id = ?2
               AND o.phase = 'outcome_unknown' AND o.transition_version = ?3
               AND o.input_effect_batch_id IS NULL AND o.outcome_json IS NULL
               AND EXISTS (
                   SELECT 1 FROM effects AS unknown_effect
                   WHERE unknown_effect.batch_id = b.batch_id
                     AND unknown_effect.status = 'outcome_unknown'
                     AND unknown_effect.outcome_json IS NULL
               )
               AND NOT EXISTS (
                   SELECT 1 FROM effects AS unfinished_effect
                   WHERE unfinished_effect.batch_id = b.batch_id
                     AND unfinished_effect.status NOT IN ('settled', 'outcome_unknown')
               )",
            params![
                session_id.to_string(),
                operation_id.to_string(),
                expected_transition,
            ],
            |_| Ok(()),
        )
        .optional()
        .map_err(sqlite_error)?
        .is_some();
    if batch_is_unknown {
        Ok(())
    } else {
        Err(KernelError::Corrupt(
            "unknown effect batch changed before abandonment".to_owned(),
        ))
    }
}

fn load_pending_abandonment(
    transaction: &rusqlite::Transaction<'_>,
    agent_id: AgentId,
    session_id: SessionId,
    operation_id: OperationId,
    active_operation_id: Option<&str>,
    stored: StoredOperation,
) -> Result<UnknownEffectState, KernelError> {
    if active_operation_id.map(parse_operation_id).transpose()? != Some(operation_id) {
        return Err(KernelError::Corrupt(
            "unknown-effect operation is not the session's active operation".to_owned(),
        ));
    }
    if stored.input_effect_batch_id.is_some() || stored.outcome.is_some() {
        return Err(KernelError::Corrupt(
            "unknown-effect operation contains settled input or a terminal outcome".to_owned(),
        ));
    }
    let manifest = stored.manifest.ok_or_else(|| {
        KernelError::Corrupt("unknown-effect operation has no manifest".to_owned())
    })?;
    let checkpoint = stored.checkpoint.ok_or_else(|| {
        KernelError::Corrupt("unknown-effect operation has no checkpoint".to_owned())
    })?;
    let batch_id = stored.current_effect_batch_id.ok_or_else(|| {
        KernelError::Corrupt("unknown-effect operation has no current effect batch".to_owned())
    })?;
    let effect_batch =
        load_effect_batch_facts(transaction, operation_id, batch_id, Some(&manifest))?;
    if !effect_batch
        .effects
        .iter()
        .any(|effect| matches!(effect, EffectFact::OutcomeUnknown(_)))
        || !effect_batch.effects.iter().all(|effect| {
            matches!(
                effect,
                EffectFact::Settled(_) | EffectFact::OutcomeUnknown(_)
            )
        })
    {
        return Err(KernelError::Corrupt(
            "unknown operation and effect batch states disagree".to_owned(),
        ));
    }
    let page = load_event_page(transaction, session_id, EventCursor::START)?;
    Ok(UnknownEffectState::Pending(Box::new(PendingAbandonment {
        input: UnknownEffectInput {
            agent_id,
            session_id,
            operation_id,
            command: stored.command,
            events: page.events,
            checkpoint,
            effect_batch,
        },
        manifest,
        transition_version: stored.transition_version,
        event_high_water: page.next_cursor.next_sequence(),
    })))
}

fn load_prior_abandonment(
    transaction: &rusqlite::Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
    active_operation_id: Option<&str>,
    stored: StoredOperation,
) -> Result<UnknownEffectState, KernelError> {
    let mut statement = transaction
        .prepare(
            "SELECT DISTINCT b.batch_id
             FROM effect_batches AS b
             JOIN effects AS e ON e.batch_id = b.batch_id
             WHERE b.operation_id = ?1 AND e.status = 'outcome_unknown'
             ORDER BY b.position",
        )
        .map_err(sqlite_error)?;
    let mut rows = statement
        .query([operation_id.to_string()])
        .map_err(sqlite_error)?;
    let Some(batch) = rows.next().map_err(sqlite_error)? else {
        return Err(KernelError::NoUnknownEffect(operation_id));
    };
    let batch_id = batch.get::<_, String>(0).map_err(sqlite_error)?;
    if rows.next().map_err(sqlite_error)?.is_some() {
        return Err(KernelError::Corrupt(
            "abandoned operation has more than one unknown effect batch".to_owned(),
        ));
    }
    drop(rows);
    drop(statement);
    if active_operation_id.map(parse_operation_id).transpose()? == Some(operation_id)
        || stored.current_effect_batch_id.is_some()
        || stored.input_effect_batch_id.is_some()
    {
        return Err(KernelError::Corrupt(
            "abandoned operation still owns active execution state".to_owned(),
        ));
    }
    let manifest = stored.manifest.ok_or_else(|| {
        KernelError::Corrupt("unknown-effect operation has no manifest".to_owned())
    })?;
    let batch_id = parse_effect_batch_id(&batch_id)?;
    let facts = load_effect_batch_facts(transaction, operation_id, batch_id, Some(&manifest))?;
    if !facts
        .effects
        .iter()
        .any(|effect| matches!(effect, EffectFact::OutcomeUnknown(_)))
        || !facts.effects.iter().all(|effect| {
            matches!(
                effect,
                EffectFact::Settled(_) | EffectFact::OutcomeUnknown(_)
            )
        })
    {
        return Err(KernelError::Corrupt(
            "abandoned unknown effect batch has invalid child state".to_owned(),
        ));
    }
    stored.checkpoint.ok_or_else(|| {
        KernelError::Corrupt("unknown-effect operation has no checkpoint".to_owned())
    })?;
    let outcome = stored
        .outcome
        .ok_or_else(|| KernelError::Corrupt("abandoned operation has no outcome".to_owned()))?;
    // The stored reason is display prose rather than durable identity: a retry
    // validates the abandonment's shape and returns the stored outcome
    // unchanged, so a row written before a wording change stays idempotent.
    if !matches!(outcome, OperationOutcome::Failed { .. }) {
        return Err(KernelError::Corrupt(
            "unknown effect was released without the abandonment outcome".to_owned(),
        ));
    }
    load_event_page(transaction, session_id, EventCursor::START)?;
    Ok(UnknownEffectState::AlreadyAbandoned { manifest, outcome })
}

fn require_runtime(manifest: &RuntimeManifest, runtime: &Runtime) -> Result<(), KernelError> {
    if manifest == runtime.manifest() {
        Ok(())
    } else {
        Err(KernelError::RuntimeMismatch)
    }
}

fn abandoned_outcome() -> OperationOutcome {
    OperationOutcome::Failed {
        reason: ABANDONED_REASON.to_owned(),
    }
}
