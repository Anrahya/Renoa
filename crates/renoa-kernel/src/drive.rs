use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    AgentId, Checkpoint, Command, DriveResult, EffectCompletion, EffectId, EventCursor, Kernel,
    KernelError, LoopDecision, LoopInput, OperationId, Runtime, RuntimeManifest, SemanticEvent,
    SessionId,
    admission::{parse_agent_id, parse_operation_id},
    decision_store::CommittedDecision,
    effect_store::{EffectIntentCommit, EffectStart, NewEffectIntent},
    effect_supervision::{SessionDriveLease, supervise_effect},
    operation_phase::OperationPhase,
    operation_store::load_operation,
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
    /// [`Kernel::request_cancellation`] instead persists an exact user request,
    /// signals this operation, waits for the same cleanup rule, and lets the
    /// loop close its semantic state before this method returns.
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
        lease.bind(active.operation_id)?;
        if &active.manifest != runtime.manifest() {
            return Err(KernelError::RuntimeMismatch);
        }
        #[cfg(test)]
        if active.newly_activated {
            self.crash_if(crate::CrashPoint::ActivationCommitted);
        }
        loop {
            if let Some(outcome) =
                self.close_requested_cancellation(session_id, active.operation_id, runtime)?
            {
                return Ok(DriveResult::Finished {
                    operation_id: active.operation_id,
                    outcome,
                });
            }
            let events = self.events_after(session_id, EventCursor::START)?.events;
            if matches!(
                active.phase,
                OperationPhase::EffectIntent | OperationPhase::EffectDispatched
            ) {
                self.drive_effect_phase(&active, runtime, &lease).await?;
                active = self.load_active(session_id, active.operation_id)?;
                continue;
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
            if let Some(outcome) =
                self.apply_loop_decision(session_id, &active, runtime, decision)?
            {
                #[cfg(test)]
                self.crash_if(crate::CrashPoint::TerminalCommitted);
                return Ok(DriveResult::Finished {
                    operation_id: active.operation_id,
                    outcome,
                });
            }
            active = self.load_active(session_id, active.operation_id)?;
        }
    }

    fn apply_loop_decision(
        &self,
        session_id: SessionId,
        active: &ActiveOperation,
        runtime: &Runtime,
        decision: LoopDecision,
    ) -> Result<Option<crate::OperationOutcome>, KernelError> {
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
            if self.commit_effect_intent(active.operation_id, active.transition_version, &intent)?
                == EffectIntentCommit::Committed
            {
                #[cfg(test)]
                self.crash_if(crate::CrashPoint::EffectIntentCommitted);
            }
            return Ok(None);
        }
        match self.commit_decision(
            session_id,
            active.operation_id,
            active.transition_version,
            decision,
        )? {
            CommittedDecision::Continue | CommittedDecision::CancellationPending => Ok(None),
            CommittedDecision::Finished(outcome) => Ok(Some(outcome)),
        }
    }

    async fn drive_effect_phase(
        &self,
        active: &ActiveOperation,
        runtime: &Runtime,
        lease: &SessionDriveLease,
    ) -> Result<(), KernelError> {
        let pending = match self.prepare_effect(active.operation_id, active.transition_version)? {
            EffectStart::Invoke(pending) => pending,
            EffectStart::Blocked | EffectStart::CancellationPending => return Ok(()),
        };
        if active.manifest.effect_bindings.get(&pending.binding) != Some(&pending.binding_revision)
        {
            return Err(KernelError::Corrupt(
                "active effect differs from the frozen manifest".to_owned(),
            ));
        }
        let adapter = runtime
            .resolve_effect(&pending.binding, &pending.binding_revision)
            .ok_or_else(|| KernelError::EffectBindingUnavailable(pending.binding.clone()))?;
        let executor =
            tokio::runtime::Handle::try_current().map_err(|_| KernelError::RuntimeUnavailable)?;
        let effect_id = pending.effect_id;
        let expected_transition = pending.transition_version;
        #[cfg(test)]
        self.crash_if(crate::CrashPoint::EffectDispatchCommitted);
        let invocation =
            pending.into_invocation(active.manifest.clone(), lease.effect_cancellation());
        let completion = supervise_effect(&executor, adapter, invocation, lease.clone()).await?;
        #[cfg(test)]
        self.crash_if(crate::CrashPoint::EffectCompletedBeforeSettlement);
        let EffectCompletion::Settled(outcome) = completion else {
            self.record_outcome_unknown(active.operation_id, effect_id, expected_transition)?;
            return Ok(());
        };
        self.settle_effect(
            active.operation_id,
            effect_id,
            expected_transition,
            &outcome,
        )?;
        #[cfg(test)]
        self.crash_if(crate::CrashPoint::EffectSettlementCommitted);
        Ok(())
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
            runtime_manifest: active.manifest.clone(),
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
        let mut active = load_active_query(&transaction, agent_id, session_id, operation_id)?;
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
        load_active_query(
            &connection,
            parse_agent_id(&agent_id)?,
            session_id,
            operation_id,
        )
    }
}

fn load_active_query(
    connection: &rusqlite::Connection,
    agent_id: AgentId,
    session_id: SessionId,
    operation_id: OperationId,
) -> Result<ActiveOperation, KernelError> {
    let stored = load_operation(connection, session_id, operation_id)?
        .ok_or_else(|| KernelError::Corrupt("active operation is missing".to_owned()))?;
    let manifest = stored.manifest.ok_or_else(|| {
        KernelError::Corrupt("active operation has no runtime manifest".to_owned())
    })?;
    Ok(ActiveOperation {
        agent_id,
        operation_id,
        command: stored.command,
        manifest,
        checkpoint: stored.checkpoint,
        transition_version: stored.transition_version,
        phase: stored.phase,
        input_effect_id: stored.input_effect_id,
        newly_activated: false,
    })
}
