use rusqlite::{OptionalExtension, TransactionBehavior};

use crate::{
    CommandId, Kernel, KernelError, OperationSnapshot, SessionId, SessionSnapshot,
    admission::{UNFINISHED_OPERATION, from_sql_integer, parse_agent_id, parse_operation_id},
    effect_store::load_effect_batch_snapshots,
    operation_store::StoredOperationRow,
    schema::sqlite_error,
};

impl Kernel {
    /// Reads one transactionally consistent session snapshot.
    ///
    /// # Errors
    ///
    /// Returns a not-found, compatibility, corruption, or storage failure.
    pub fn inspect(&self, session_id: SessionId) -> Result<SessionSnapshot, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(sqlite_error)?;
        let agent_id = transaction
            .query_row(
                "SELECT agent_id FROM sessions WHERE session_id = ?1",
                [session_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or(KernelError::SessionNotFound(session_id))?;
        let mut statement = transaction
            .prepare(
                "SELECT o.operation_id, o.position,
                        o.command_id, c.content_json, o.phase, o.state_version,
                        o.transition_version, o.manifest_json, o.checkpoint_json,
                        o.current_effect_batch_id, o.input_effect_batch_id, o.outcome_json
                 FROM operations AS o
                 JOIN commands AS c
                   ON c.session_id = o.session_id AND c.command_id = o.command_id
                 WHERE o.session_id = ?1 ORDER BY o.position",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([session_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    StoredOperationRow::read(row, 2)?,
                ))
            })
            .map_err(sqlite_error)?;
        let mut operations = Vec::new();
        for row in rows {
            let (operation_id, position, stored) = row.map_err(sqlite_error)?;
            let operation_id = parse_operation_id(&operation_id)?;
            let stored = stored.decode()?;
            let command_id = stored.command.command_id();
            operations.push(OperationSnapshot {
                operation_id,
                command_id,
                command: stored.command,
                position: from_sql_integer(position, "operation position")?,
                status: stored.phase.status(),
                manifest: stored.manifest,
                checkpoint: stored.checkpoint,
                outcome: stored.outcome,
                effect_batches: load_effect_batch_snapshots(&transaction, operation_id)?,
            });
        }
        drop(statement);
        transaction.commit().map_err(sqlite_error)?;
        Ok(SessionSnapshot {
            agent_id: parse_agent_id(&agent_id)?,
            operations,
        })
    }

    /// Where `command_id` stands in `session_id`, from one read instead of a
    /// whole snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::SessionNotFound`] for a session the kernel does
    /// not hold, or a storage failure.
    pub fn command_standing(
        &self,
        session_id: SessionId,
        command_id: CommandId,
    ) -> Result<CommandStanding, KernelError> {
        let connection = self.database.connection()?;
        let (admitted, other_unfinished) = connection
            .query_row(
                &format!(
                    "SELECT
                        EXISTS (SELECT 1 FROM operations
                                WHERE session_id = ?1 AND command_id = ?2),
                        EXISTS (SELECT 1 FROM operations
                                WHERE session_id = ?1 AND command_id <> ?2
                                  AND {UNFINISHED_OPERATION})
                     FROM sessions WHERE session_id = ?1"
                ),
                [session_id.to_string(), command_id.to_string()],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or(KernelError::SessionNotFound(session_id))?;
        Ok(CommandStanding {
            admitted,
            other_unfinished,
        })
    }
}

/// Where one command stands in its session before a caller resolves its
/// runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandStanding {
    /// The kernel holds an operation for this command.
    pub admitted: bool,
    /// Another operation in the session has no outcome, which exclusive
    /// admission refuses this command for.
    pub other_unfinished: bool,
}
