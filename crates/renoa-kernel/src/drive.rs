use rusqlite::{OptionalExtension, TransactionBehavior, params};
use tokio_util::sync::CancellationToken;

use crate::{
    AgentId, Checkpoint, Command, DriveResult, EffectCompletion, EffectId, EventCursor, Kernel,
    KernelError, LoopDecision, LoopInput, OperationId, Runtime, RuntimeManifest, SemanticEvent,
    SessionId,
    admission::{parse_agent_id, parse_operation_id},
    decision_store::CommittedDecision,
    effect_store::{EffectStart, NewEffectIntent, parse_effect_id},
    effect_supervision::{SessionDriveLease, supervise_effect},
    inspection::require_state_version,
    operation_phase::OperationPhase,
    runtime::require_compatible_checkpoint,
    schema::{json_error, sqlite_error},
};

struct ActiveOperation {
    agent_id: AgentId,
    operation_id: OperationId,
    command: Command,
    manifest: RuntimeManifest,
    checkpoint: Option<Checkpoint>,
    transition_version: i64,
    phase: OperationPhase,
    input_effect_id: Option<EffectId>,
    newly_activated: bool,
}

enum EffectAdvance {
    Settled,
    Blocked,
}

impl Kernel {
    /// Activates and drives one ordered operation to a terminal or blocked boundary.
    ///
    /// # Errors
    ///
    /// Fails closed on ownership, runtime, checkpoint, loop, or durable-state
    /// incompatibility before starting an external effect.
    /// Effect execution also requires this future to be polled inside a Tokio
    /// runtime.
    ///
    /// # Cancellation
    ///
    /// Dropping this future cancels any in-flight adapter invocation. The
    /// process-local session lease and database writer lease remain held until
    /// the adapter resolves after cleanup. The next drive then applies the
    /// persisted recovery class if no outcome was durably settled.
    ///
    /// # Panics
    ///
    /// Propagates a panic from a loop plugin or effect adapter. Durable state
    /// remains at the last committed recovery boundary.
    pub async fn drive(
        &self,
        session_id: SessionId,
        runtime: &Runtime,
    ) -> Result<DriveResult, KernelError> {
        let lease = SessionDriveLease::acquire(&self.running_sessions, &self.database, session_id)?;
        let Some(mut active) = self.activate(session_id, runtime.manifest())? else {
            return Ok(DriveResult::Idle);
        };
        if &active.manifest != runtime.manifest() {
            return Err(KernelError::RuntimeMismatch);
        }
        #[cfg(test)]
        if active.newly_activated {
            self.crash_if(crate::CrashPoint::ActivationCommitted);
        }
        loop {
            let events = self.events_after(session_id, EventCursor::START)?.events;
            if matches!(
                active.phase,
                OperationPhase::EffectIntent | OperationPhase::EffectDispatched
            ) {
                match self.drive_effect_phase(&active, runtime, &lease).await? {
                    EffectAdvance::Blocked => {
                        return Ok(DriveResult::Blocked {
                            operation_id: active.operation_id,
                        });
                    }
                    EffectAdvance::Settled => {
                        active = self.load_active(session_id, active.operation_id)?;
                        continue;
                    }
                }
            }
            if active.phase == OperationPhase::OutcomeUnknown {
                return Ok(DriveResult::Blocked {
                    operation_id: active.operation_id,
                });
            }
            if active.phase != OperationPhase::NeedDecision {
                return Err(KernelError::Corrupt(format!(
                    "unsupported active phase `{}`",
                    active.phase.as_str()
                )));
            }
            let input = self.load_loop_input(session_id, &active, events)?;
            let decision = runtime.plugin.decide(input).map_err(KernelError::Loop)?;
            let found = decision.checkpoint().schema_version();
            let expected = active.manifest.checkpoint_schema_version;
            if found != expected {
                return Err(KernelError::CheckpointSchemaMismatch { expected, found });
            }
            if let LoopDecision::InvokeEffect {
                checkpoint,
                binding,
                request,
                recovery,
            } = decision
            {
                let revision = active
                    .manifest
                    .effect_bindings
                    .get(&binding)
                    .ok_or_else(|| KernelError::EffectBindingUnavailable(binding.clone()))?;
                runtime
                    .resolve_effect(&binding, revision)
                    .ok_or_else(|| KernelError::EffectBindingUnavailable(binding.clone()))?;
                let intent = NewEffectIntent {
                    checkpoint,
                    binding,
                    binding_revision: revision.clone(),
                    request,
                    recovery,
                };
                self.commit_effect_intent(active.operation_id, active.transition_version, &intent)?;
                #[cfg(test)]
                self.crash_if(crate::CrashPoint::EffectIntentCommitted);
                active = self.load_active(session_id, active.operation_id)?;
                continue;
            }
            match self.commit_decision(
                session_id,
                active.operation_id,
                active.transition_version,
                decision,
            )? {
                CommittedDecision::Continue => {
                    active = self.load_active(session_id, active.operation_id)?;
                }
                CommittedDecision::Finished(outcome) => {
                    #[cfg(test)]
                    self.crash_if(crate::CrashPoint::TerminalCommitted);
                    return Ok(DriveResult::Finished {
                        operation_id: active.operation_id,
                        outcome,
                    });
                }
            }
        }
    }

    async fn drive_effect_phase(
        &self,
        active: &ActiveOperation,
        runtime: &Runtime,
        lease: &SessionDriveLease,
    ) -> Result<EffectAdvance, KernelError> {
        let (binding, revision) = self
            .load_current_effect_binding(active.operation_id)?
            .ok_or_else(|| KernelError::Corrupt("active effect binding is missing".to_owned()))?;
        if active.manifest.effect_bindings.get(&binding) != Some(&revision) {
            return Err(KernelError::Corrupt(
                "active effect differs from the frozen manifest".to_owned(),
            ));
        }
        let adapter = runtime
            .resolve_effect(&binding, &revision)
            .ok_or_else(|| KernelError::EffectBindingUnavailable(binding.clone()))?;
        let executor =
            tokio::runtime::Handle::try_current().map_err(|_| KernelError::RuntimeUnavailable)?;
        let EffectStart::Invoke(pending) =
            self.prepare_effect(active.operation_id, active.transition_version)?
        else {
            return Ok(EffectAdvance::Blocked);
        };
        if pending.binding != binding || pending.binding_revision != revision {
            return Err(KernelError::Corrupt(
                "prepared effect changed its frozen binding".to_owned(),
            ));
        }
        let effect_id = pending.effect_id;
        let expected_transition = pending.transition_version;
        #[cfg(test)]
        self.crash_if(crate::CrashPoint::EffectDispatchCommitted);
        let cancellation = CancellationToken::new();
        let invocation = pending.into_invocation(cancellation);
        let completion = supervise_effect(&executor, adapter, invocation, lease.clone()).await?;
        #[cfg(test)]
        self.crash_if(crate::CrashPoint::EffectCompletedBeforeSettlement);
        let EffectCompletion::Settled(outcome) = completion else {
            self.record_outcome_unknown(active.operation_id, effect_id, expected_transition)?;
            return Ok(EffectAdvance::Blocked);
        };
        self.settle_effect(
            active.operation_id,
            effect_id,
            expected_transition,
            &outcome,
        )?;
        #[cfg(test)]
        self.crash_if(crate::CrashPoint::EffectSettlementCommitted);
        Ok(EffectAdvance::Settled)
    }

    fn load_loop_input(
        &self,
        session_id: SessionId,
        active: &ActiveOperation,
        events: Vec<SemanticEvent>,
    ) -> Result<LoopInput, KernelError> {
        let effect = active
            .input_effect_id
            .map(|effect_id| self.load_settled_effect(effect_id, active.operation_id))
            .transpose()?;
        if let Some(effect) = effect.as_ref()
            && active.manifest.effect_bindings.get(&effect.binding)
                != Some(&effect.binding_revision)
        {
            return Err(KernelError::Corrupt(
                "settled effect differs from the frozen manifest".to_owned(),
            ));
        }
        Ok(LoopInput {
            agent_id: active.agent_id,
            session_id,
            operation_id: active.operation_id,
            command: active.command.clone(),
            events,
            checkpoint: active.checkpoint.clone(),
            effect,
        })
    }

    fn activate(
        &self,
        session_id: SessionId,
        manifest: &RuntimeManifest,
    ) -> Result<Option<ActiveOperation>, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let (agent_id, active_id) = transaction
            .query_row(
                "SELECT agent_id, active_operation_id FROM sessions WHERE session_id = ?1",
                [session_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or(KernelError::SessionNotFound(session_id))?;
        let agent_id = parse_agent_id(&agent_id)?;
        let (operation_id, newly_activated) = if let Some(active_id) = active_id {
            (parse_operation_id(&active_id)?, false)
        } else {
            let queued = transaction
                .query_row(
                    "SELECT operation_id FROM operations
                     WHERE session_id = ?1 AND phase = 'queued'
                     ORDER BY position LIMIT 1",
                    [session_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?;
            let Some(queued) = queued else {
                transaction.commit().map_err(sqlite_error)?;
                return Ok(None);
            };
            let operation_id = parse_operation_id(&queued)?;
            let manifest_json = serde_json::to_string(manifest).map_err(json_error)?;
            let changed = transaction
                .execute(
                    "UPDATE operations
                     SET phase = 'need_decision', manifest_json = ?2,
                         transition_version = transition_version + 1
                     WHERE operation_id = ?1 AND phase = 'queued'
                         AND manifest_json IS NULL",
                    params![operation_id.to_string(), manifest_json],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(KernelError::Corrupt(
                    "operation activation compare-and-set failed".to_owned(),
                ));
            }
            let changed = transaction
                .execute(
                    "UPDATE sessions SET active_operation_id = ?2
                     WHERE session_id = ?1 AND active_operation_id IS NULL",
                    params![session_id.to_string(), operation_id.to_string()],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(KernelError::Corrupt(
                    "session activation compare-and-set failed".to_owned(),
                ));
            }
            (operation_id, true)
        };
        let mut active = load_active_query(&transaction, agent_id, operation_id)?;
        active.newly_activated = newly_activated;
        transaction.commit().map_err(sqlite_error)?;
        Ok(Some(active))
    }

    fn load_active(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Result<ActiveOperation, KernelError> {
        let connection = self.database.connection()?;
        let agent_id = connection
            .query_row(
                "SELECT agent_id FROM sessions
                 WHERE session_id = ?1 AND active_operation_id = ?2",
                params![session_id.to_string(), operation_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or_else(|| KernelError::Corrupt("active operation pointer changed".to_owned()))?;
        load_active_query(&connection, parse_agent_id(&agent_id)?, operation_id)
    }
}

fn load_active_query(
    connection: &rusqlite::Connection,
    agent_id: AgentId,
    operation_id: OperationId,
) -> Result<ActiveOperation, KernelError> {
    let (
        command_json,
        command_id,
        phase,
        state_version,
        transition_version,
        manifest_json,
        checkpoint_json,
        input_effect_id,
    ) = connection
        .query_row(
            "SELECT c.content_json, o.command_id, o.phase, o.state_version,
                        o.transition_version, o.manifest_json,
                        o.checkpoint_json, o.input_effect_id
                 FROM operations AS o
                 JOIN commands AS c ON c.command_id = o.command_id
                 WHERE o.operation_id = ?1",
            [operation_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| KernelError::Corrupt("active operation is missing".to_owned()))?;
    require_state_version(state_version)?;
    let phase = OperationPhase::from_database(&phase)?;
    let manifest_json = manifest_json.ok_or_else(|| {
        KernelError::Corrupt("active operation has no runtime manifest".to_owned())
    })?;
    let manifest: RuntimeManifest = serde_json::from_str(&manifest_json).map_err(json_error)?;
    let checkpoint: Option<Checkpoint> = checkpoint_json
        .map(|value| serde_json::from_str(&value).map_err(json_error))
        .transpose()?;
    require_compatible_checkpoint(&manifest, checkpoint.as_ref())?;
    let command: Command = serde_json::from_str(&command_json).map_err(json_error)?;
    let stored_command_id = crate::admission::parse_command_id(&command_id)?;
    if command.command_id() != stored_command_id {
        return Err(KernelError::Corrupt(
            "active operation command identity differs from stored content".to_owned(),
        ));
    }
    Ok(ActiveOperation {
        agent_id,
        operation_id,
        command,
        manifest,
        checkpoint,
        transition_version,
        phase,
        input_effect_id: input_effect_id
            .map(|value| parse_effect_id(&value))
            .transpose()?,
        newly_activated: false,
    })
}
