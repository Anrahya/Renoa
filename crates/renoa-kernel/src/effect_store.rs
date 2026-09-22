use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    Checkpoint, EffectBatchId, EffectId, EffectInvocation, EffectOutcome, EffectRecovery,
    EffectStatus, Kernel, KernelError, OperationId, RuntimeManifest,
    admission::from_sql_integer,
    cancellation::cancellation_requested,
    operation_phase::OperationPhase,
    schema::{json_error, sqlite_error},
};

mod dispatch;
mod projection;

pub(crate) use projection::{load_effect_batch_facts, load_effect_batch_snapshots};

pub(crate) struct PendingEffect {
    pub(crate) batch_id: EffectBatchId,
    pub(crate) effect_id: EffectId,
    pub(crate) binding: String,
    pub(crate) binding_revision: String,
    pub(crate) request: Value,
    pub(crate) recovery: EffectRecovery,
    /// Persisted dispatch count after the dispatch this value describes.
    pub(crate) dispatch_count: u64,
    pub(crate) transition_version: i64,
}

pub(crate) struct PendingEffectBatch {
    pub(crate) batch_id: EffectBatchId,
    pub(crate) effects: Vec<PendingEffect>,
}

pub(crate) struct NewEffectIntent {
    pub(crate) binding: String,
    pub(crate) binding_revision: String,
    pub(crate) request: Value,
    pub(crate) recovery: EffectRecovery,
}

pub(crate) struct NewEffectBatchIntent {
    pub(crate) checkpoint: Checkpoint,
    pub(crate) effects: Vec<NewEffectIntent>,
}

impl PendingEffect {
    pub(crate) fn into_invocation(
        self,
        runtime_manifest: RuntimeManifest,
        cancellation: CancellationToken,
    ) -> EffectInvocation {
        EffectInvocation {
            batch_id: self.batch_id,
            effect_id: self.effect_id,
            binding: self.binding,
            binding_revision: self.binding_revision,
            runtime_manifest,
            request: self.request,
            cancellation,
        }
    }
}

pub(crate) enum EffectBatchStart {
    Invoke(PendingEffectBatch),
    Settled,
    Blocked,
    CancellationPending,
}

pub(crate) enum EffectBatchFinish {
    Retry,
    Settled,
    Blocked,
    CancellationPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectIntentCommit {
    Committed,
    CancellationPending,
}

impl Kernel {
    pub(crate) fn settle_batch_effect(
        &self,
        operation_id: OperationId,
        batch_id: EffectBatchId,
        effect_id: EffectId,
        expected_transition: i64,
        outcome: &EffectOutcome,
    ) -> Result<bool, KernelError> {
        let outcome_json = serde_json::to_string(outcome).map_err(json_error)?;
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        require_dispatched_batch(&transaction, operation_id, batch_id, expected_transition)?;
        let changed = transaction
            .execute(
                "UPDATE effects SET status = 'settled', outcome_json = ?3
                 WHERE effect_id = ?1 AND batch_id = ?2
                   AND status = 'dispatch_started'",
                params![effect_id.to_string(), batch_id.to_string(), outcome_json],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "effect settlement compare-and-set failed".to_owned(),
            ));
        }
        let finished = transition_batch_if_terminal(
            &transaction,
            operation_id,
            batch_id,
            expected_transition,
        )?
        .is_some();
        transaction.commit().map_err(sqlite_error)?;
        Ok(finished)
    }

    pub(crate) fn record_batch_effect_unknown(
        &self,
        operation_id: OperationId,
        batch_id: EffectBatchId,
        effect_id: EffectId,
        expected_transition: i64,
    ) -> Result<bool, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        require_dispatched_batch(&transaction, operation_id, batch_id, expected_transition)?;
        update_effect_status(
            &transaction,
            effect_id,
            EffectStatus::DispatchStarted,
            EffectStatus::OutcomeUnknown,
        )?;
        let finished = transition_batch_if_terminal(
            &transaction,
            operation_id,
            batch_id,
            expected_transition,
        )?
        .is_some();
        transaction.commit().map_err(sqlite_error)?;
        Ok(finished)
    }

    pub(crate) fn finish_effect_batch_attempt(
        &self,
        operation_id: OperationId,
        batch_id: EffectBatchId,
        expected_transition: i64,
    ) -> Result<EffectBatchFinish, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        require_dispatched_batch(&transaction, operation_id, batch_id, expected_transition)?;
        if cancellation_requested(&transaction, operation_id)? {
            transaction.commit().map_err(sqlite_error)?;
            return Ok(EffectBatchFinish::CancellationPending);
        }
        let Some(blocked) = transition_batch_if_terminal(
            &transaction,
            operation_id,
            batch_id,
            expected_transition,
        )?
        else {
            transaction.commit().map_err(sqlite_error)?;
            return Ok(EffectBatchFinish::Retry);
        };
        transaction.commit().map_err(sqlite_error)?;
        Ok(if blocked {
            EffectBatchFinish::Blocked
        } else {
            EffectBatchFinish::Settled
        })
    }

    pub(crate) fn load_settled_effect_batch(
        &self,
        batch_id: EffectBatchId,
        operation_id: OperationId,
    ) -> Result<crate::SettledEffectBatch, KernelError> {
        let connection = self.database.connection()?;
        projection::load_settled_effect_batch(&connection, operation_id, batch_id)
    }
}

fn transition_batch_if_terminal(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
    batch_id: EffectBatchId,
    expected_transition: i64,
) -> Result<Option<bool>, KernelError> {
    let statuses = load_batch_statuses(transaction, operation_id, batch_id)?;
    if statuses.contains(&EffectStatus::IntentCommitted) {
        return Err(KernelError::Corrupt(
            "dispatched effect batch contains an unmarked child".to_owned(),
        ));
    }
    if statuses.contains(&EffectStatus::DispatchStarted) {
        return Ok(None);
    }
    let blocked = statuses.contains(&EffectStatus::OutcomeUnknown);
    transition_finished_batch(
        transaction,
        operation_id,
        batch_id,
        expected_transition,
        blocked,
    )?;
    Ok(Some(blocked))
}

pub(crate) fn close_dispatched_batch_for_cancellation(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
    batch_id: EffectBatchId,
    expected_transition: i64,
) -> Result<OperationPhase, KernelError> {
    require_dispatched_batch(transaction, operation_id, batch_id, expected_transition)?;
    transaction
        .execute(
            "UPDATE effects SET status = 'outcome_unknown'
             WHERE batch_id = ?1 AND status = 'dispatch_started'",
            [batch_id.to_string()],
        )
        .map_err(sqlite_error)?;
    let statuses = load_batch_statuses(transaction, operation_id, batch_id)?;
    if statuses.contains(&EffectStatus::IntentCommitted)
        || statuses.contains(&EffectStatus::DispatchStarted)
    {
        return Err(KernelError::Corrupt(
            "cancellation could not close every dispatched child".to_owned(),
        ));
    }
    let blocked = statuses.contains(&EffectStatus::OutcomeUnknown);
    transition_finished_batch(
        transaction,
        operation_id,
        batch_id,
        expected_transition,
        blocked,
    )?;
    Ok(if blocked {
        OperationPhase::OutcomeUnknown
    } else {
        OperationPhase::NeedDecision
    })
}

fn transition_finished_batch(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
    batch_id: EffectBatchId,
    expected_transition: i64,
    blocked: bool,
) -> Result<(), KernelError> {
    let (phase, current_batch, input_batch): (&str, Option<String>, Option<String>) = if blocked {
        ("outcome_unknown", Some(batch_id.to_string()), None)
    } else {
        ("need_decision", None, Some(batch_id.to_string()))
    };
    let changed = transaction
        .execute(
            "UPDATE operations
             SET phase = ?4, current_effect_batch_id = ?5,
                 input_effect_batch_id = ?6,
                 transition_version = transition_version + 1
             WHERE operation_id = ?1 AND phase = 'effect_dispatched'
               AND transition_version = ?2 AND current_effect_batch_id = ?3",
            params![
                operation_id.to_string(),
                expected_transition,
                batch_id.to_string(),
                phase,
                current_batch,
                input_batch,
            ],
        )
        .map_err(sqlite_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(KernelError::Corrupt(
            "effect batch completion compare-and-set failed".to_owned(),
        ))
    }
}

fn require_dispatched_batch(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
    batch_id: EffectBatchId,
    expected_transition: i64,
) -> Result<(), KernelError> {
    let active = transaction
        .query_row(
            "SELECT 1 FROM operations
             WHERE operation_id = ?1 AND phase = 'effect_dispatched'
               AND transition_version = ?2 AND current_effect_batch_id = ?3",
            params![
                operation_id.to_string(),
                expected_transition,
                batch_id.to_string(),
            ],
            |_| Ok(()),
        )
        .optional()
        .map_err(sqlite_error)?
        .is_some();
    if active {
        Ok(())
    } else {
        Err(KernelError::Corrupt(
            "effect batch changed before child settlement".to_owned(),
        ))
    }
}

fn update_effect_status(
    transaction: &rusqlite::Transaction<'_>,
    effect_id: EffectId,
    expected: EffectStatus,
    next: EffectStatus,
) -> Result<(), KernelError> {
    let changed = transaction
        .execute(
            "UPDATE effects SET status = ?3
             WHERE effect_id = ?1 AND status = ?2",
            params![
                effect_id.to_string(),
                status_name(expected),
                status_name(next)
            ],
        )
        .map_err(sqlite_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(KernelError::Corrupt(
            "effect status compare-and-set failed".to_owned(),
        ))
    }
}

fn load_batch_statuses(
    connection: &rusqlite::Connection,
    operation_id: OperationId,
    batch_id: EffectBatchId,
) -> Result<Vec<EffectStatus>, KernelError> {
    let mut statement = connection
        .prepare(
            "SELECT e.position, e.status
             FROM effect_batches AS b
             JOIN effects AS e ON e.batch_id = b.batch_id
             WHERE b.operation_id = ?1 AND b.batch_id = ?2
             ORDER BY e.position",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(
            params![operation_id.to_string(), batch_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(sqlite_error)?;
    let mut statuses = Vec::new();
    for (expected_position, row) in rows.enumerate() {
        let (position, status) = row.map_err(sqlite_error)?;
        if from_sql_integer(position, "effect position")? != expected_position as u64 {
            return Err(KernelError::Corrupt(
                "effect batch child positions are not gapless".to_owned(),
            ));
        }
        statuses.push(parse_status(&status)?);
    }
    if statuses.is_empty() {
        return Err(KernelError::Corrupt(
            "effect batch contains no children".to_owned(),
        ));
    }
    Ok(statuses)
}

pub(crate) fn parse_effect_batch_id(value: &str) -> Result<EffectBatchId, KernelError> {
    uuid::Uuid::parse_str(value)
        .map(EffectBatchId::from_uuid)
        .map_err(|error| KernelError::Corrupt(format!("invalid effect batch id: {error}")))
}

pub(crate) fn parse_effect_id(value: &str) -> Result<EffectId, KernelError> {
    uuid::Uuid::parse_str(value)
        .map(EffectId::from_uuid)
        .map_err(|error| KernelError::Corrupt(format!("invalid effect id: {error}")))
}

pub(crate) fn parse_recovery(value: &str) -> Result<EffectRecovery, KernelError> {
    match value {
        "safe_to_replay" => Ok(EffectRecovery::SafeToReplay),
        "never_replay" => Ok(EffectRecovery::NeverReplay),
        _ => Err(KernelError::Corrupt(format!(
            "unknown effect recovery `{value}`"
        ))),
    }
}

pub(crate) fn parse_status(value: &str) -> Result<EffectStatus, KernelError> {
    match value {
        "intent_committed" => Ok(EffectStatus::IntentCommitted),
        "dispatch_started" => Ok(EffectStatus::DispatchStarted),
        "settled" => Ok(EffectStatus::Settled),
        "outcome_unknown" => Ok(EffectStatus::OutcomeUnknown),
        _ => Err(KernelError::Corrupt(format!(
            "unknown effect status `{value}`"
        ))),
    }
}

const fn status_name(status: EffectStatus) -> &'static str {
    match status {
        EffectStatus::IntentCommitted => "intent_committed",
        EffectStatus::DispatchStarted => "dispatch_started",
        EffectStatus::Settled => "settled",
        EffectStatus::OutcomeUnknown => "outcome_unknown",
    }
}
