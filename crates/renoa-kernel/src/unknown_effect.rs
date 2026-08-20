use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    AgentId, EventCursor, Kernel, KernelError, OperationId, OperationOutcome, Runtime,
    RuntimeManifest, SessionId, UnknownEffect, UnknownEffectAbandonment, UnknownEffectInput,
    admission::{from_sql_integer, parse_agent_id, parse_operation_id},
    cancellation::cancellation_requested,
    decision_store::append_events,
    effect_store::parse_effect_id,
    effect_supervision::SessionDriveLease,
    events::{load_event_page, validate_new_events},
    operation_phase::OperationPhase,
    operation_store::{StoredOperation, load_operation},
    runtime::require_compatible_checkpoint,
    schema::{json_error, sqlite_error},
};

const ABANDONED_REASON: &str = "effect outcome is unknown; operation was abandoned without replay";

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
    /// Explicitly closes an operation whose external effect outcome is unknown.
    ///
    /// The kernel validates the exact active operation, frozen runtime, gapless
    /// semantic history, checkpoint, and effect identity before asking the loop
    /// to close its own state. The unknown effect is never invoked or rewritten
    /// as a definite outcome.
    ///
    /// Repeating this call after a committed abandonment returns the same
    /// terminal outcome without appending duplicate events.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::NoUnknownEffect`] when the operation has no
    /// unknown effect to abandon. All compatibility, ownership, corruption,
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
        let effect_is_unknown = transaction
            .query_row(
                "SELECT 1
                 FROM operations AS o
                 JOIN effects AS e
                   ON e.operation_id = o.operation_id
                  AND e.effect_id = o.current_effect_id
                 WHERE o.session_id = ?1 AND o.operation_id = ?2
                   AND o.phase = 'outcome_unknown' AND o.transition_version = ?3
                   AND o.input_effect_id IS NULL AND o.outcome_json IS NULL
                   AND e.status = 'outcome_unknown' AND e.outcome_json IS NULL",
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
        if !effect_is_unknown {
            return Err(KernelError::Corrupt(
                "unknown effect changed before abandonment".to_owned(),
            ));
        }

        append_events(&transaction, session_id, operation_id, &abandonment.events)?;
        let outcome = abandoned_outcome();
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET phase = 'failed', checkpoint_json = ?4,
                     current_effect_id = NULL, input_effect_id = NULL,
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
    if stored.input_effect_id.is_some() || stored.outcome.is_some() {
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
    let effect_id = stored.current_effect_id.ok_or_else(|| {
        KernelError::Corrupt("unknown-effect operation has no current effect".to_owned())
    })?;
    let (binding, binding_revision, request_json, status, outcome_json) = transaction
        .query_row(
            "SELECT binding, binding_revision, request_json, status, outcome_json
             FROM effects WHERE effect_id = ?1 AND operation_id = ?2",
            params![effect_id.to_string(), operation_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| KernelError::Corrupt("unknown current effect is missing".to_owned()))?;
    if status != "outcome_unknown" || outcome_json.is_some() {
        return Err(KernelError::Corrupt(
            "unknown operation and effect states disagree".to_owned(),
        ));
    }
    if manifest.effect_bindings.get(&binding) != Some(&binding_revision) {
        return Err(KernelError::Corrupt(
            "unknown effect differs from the frozen manifest".to_owned(),
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
            effect: UnknownEffect {
                effect_id,
                binding,
                binding_revision,
                request: serde_json::from_str(&request_json).map_err(json_error)?,
            },
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
            "SELECT effect_id, binding, binding_revision, request_json, outcome_json
             FROM effects
             WHERE operation_id = ?1 AND status = 'outcome_unknown'",
        )
        .map_err(sqlite_error)?;
    let mut rows = statement
        .query([operation_id.to_string()])
        .map_err(sqlite_error)?;
    let Some(effect) = rows.next().map_err(sqlite_error)? else {
        return Err(KernelError::NoUnknownEffect(operation_id));
    };
    let effect_id = effect.get::<_, String>(0).map_err(sqlite_error)?;
    let binding = effect.get::<_, String>(1).map_err(sqlite_error)?;
    let binding_revision = effect.get::<_, String>(2).map_err(sqlite_error)?;
    let request_json = effect.get::<_, String>(3).map_err(sqlite_error)?;
    let outcome_json = effect.get::<_, Option<String>>(4).map_err(sqlite_error)?;
    if rows.next().map_err(sqlite_error)?.is_some() {
        return Err(KernelError::Corrupt(
            "abandoned operation has more than one unknown effect".to_owned(),
        ));
    }
    drop(rows);
    drop(statement);
    if active_operation_id.map(parse_operation_id).transpose()? == Some(operation_id)
        || stored.current_effect_id.is_some()
        || stored.input_effect_id.is_some()
    {
        return Err(KernelError::Corrupt(
            "abandoned operation still owns active execution state".to_owned(),
        ));
    }
    let manifest = stored.manifest.ok_or_else(|| {
        KernelError::Corrupt("unknown-effect operation has no manifest".to_owned())
    })?;
    if manifest.effect_bindings.get(&binding) != Some(&binding_revision) {
        return Err(KernelError::Corrupt(
            "abandoned unknown effect differs from the frozen manifest".to_owned(),
        ));
    }
    parse_effect_id(&effect_id)?;
    if outcome_json.is_some() {
        return Err(KernelError::Corrupt(
            "abandoned unknown effect contains a definite outcome".to_owned(),
        ));
    }
    serde_json::from_str::<serde_json::Value>(&request_json).map_err(json_error)?;
    stored.checkpoint.ok_or_else(|| {
        KernelError::Corrupt("unknown-effect operation has no checkpoint".to_owned())
    })?;
    let outcome = stored
        .outcome
        .ok_or_else(|| KernelError::Corrupt("abandoned operation has no outcome".to_owned()))?;
    if outcome != abandoned_outcome() {
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
