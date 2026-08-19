use rusqlite::{OptionalExtension, TransactionBehavior};

use crate::{
    Kernel, KernelError, OperationOutcome, OperationSnapshot, SessionId, SessionSnapshot,
    admission::{from_sql_integer, parse_agent_id, parse_command_id, parse_operation_id},
    effect_store::load_effect_snapshots,
    operation_phase::OperationPhase,
    runtime::require_compatible_checkpoint,
    schema::{json_error, sqlite_error},
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
                "SELECT o.operation_id, o.command_id, o.position, o.phase, o.state_version,
                        o.manifest_json, o.checkpoint_json, o.outcome_json, c.content_json
                 FROM operations AS o
                 JOIN commands AS c ON c.command_id = o.command_id
                 WHERE o.session_id = ?1 ORDER BY o.position",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([session_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .map_err(sqlite_error)?;
        let mut operations = Vec::new();
        for row in rows {
            let (
                operation_id,
                command_id,
                position,
                phase,
                state_version,
                manifest,
                checkpoint,
                outcome,
                command_json,
            ) = row.map_err(sqlite_error)?;
            require_state_version(state_version)?;
            let operation_id = parse_operation_id(&operation_id)?;
            let command_id = parse_command_id(&command_id)?;
            let command: crate::Command =
                serde_json::from_str(&command_json).map_err(json_error)?;
            if command.command_id() != command_id {
                return Err(KernelError::Corrupt(
                    "operation command identity differs from stored content".to_owned(),
                ));
            }
            let manifest = manifest
                .map(|value| serde_json::from_str(&value).map_err(json_error))
                .transpose()?;
            let checkpoint = checkpoint
                .map(|value| serde_json::from_str(&value).map_err(json_error))
                .transpose()?;
            if let Some(manifest) = manifest.as_ref() {
                require_compatible_checkpoint(manifest, checkpoint.as_ref())?;
            } else if checkpoint.is_some() {
                return Err(KernelError::Corrupt(
                    "operation checkpoint has no runtime manifest".to_owned(),
                ));
            }
            operations.push(OperationSnapshot {
                operation_id,
                command_id,
                command,
                position: from_sql_integer(position, "operation position")?,
                status: OperationPhase::from_database(&phase)?.status(),
                manifest,
                checkpoint,
                outcome: outcome
                    .map(|value| {
                        serde_json::from_str::<OperationOutcome>(&value).map_err(json_error)
                    })
                    .transpose()?,
                effects: load_effect_snapshots(&transaction, operation_id)?,
            });
        }
        drop(statement);
        transaction.commit().map_err(sqlite_error)?;
        Ok(SessionSnapshot {
            agent_id: parse_agent_id(&agent_id)?,
            operations,
        })
    }
}

pub(crate) fn require_state_version(version: i64) -> Result<(), KernelError> {
    let supported = crate::schema::OPERATION_STATE_VERSION;
    match version.cmp(&i64::from(supported)) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Greater => Err(KernelError::UnsupportedStateVersion {
            found: u32::try_from(version).unwrap_or(u32::MAX),
            supported,
        }),
        std::cmp::Ordering::Less => Err(KernelError::Corrupt(format!(
            "invalid operation state version {version}"
        ))),
    }
}
