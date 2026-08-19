use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    Admission, AgentId, Command, Kernel, KernelError, OperationId, SessionId,
    schema::{json_error, sqlite_error},
};

impl Kernel {
    /// Ensures that an agent with this stable identity exists.
    ///
    /// # Errors
    ///
    /// Returns a storage failure when the identity cannot be committed.
    pub fn create_agent(&self, agent_id: AgentId) -> Result<(), KernelError> {
        let connection = self.database.connection()?;
        connection
            .execute(
                "INSERT INTO agents (agent_id) VALUES (?1)
                 ON CONFLICT(agent_id) DO NOTHING",
                [agent_id.to_string()],
            )
            .map_err(sqlite_error)?;
        Ok(())
    }

    /// Ensures that a session exists under exactly one agent identity.
    ///
    /// # Errors
    ///
    /// Returns a conflict if the stable session identity is already bound to
    /// another agent.
    pub fn create_session(
        &self,
        session_id: SessionId,
        agent_id: AgentId,
    ) -> Result<(), KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let existing = transaction
            .query_row(
                "SELECT agent_id FROM sessions WHERE session_id = ?1",
                [session_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?;
        if let Some(existing) = existing {
            let existing = parse_agent_id(&existing)?;
            return if existing == agent_id {
                transaction.commit().map_err(sqlite_error)
            } else {
                Err(KernelError::SessionConflict {
                    session_id,
                    agent_id: existing,
                })
            };
        }
        let agent_exists = transaction
            .query_row(
                "SELECT 1 FROM agents WHERE agent_id = ?1",
                [agent_id.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map_err(sqlite_error)?
            .is_some();
        if !agent_exists {
            return Err(KernelError::AgentNotFound(agent_id));
        }
        transaction
            .execute(
                "INSERT INTO sessions (
                    session_id, agent_id, next_command_position,
                    active_operation_id, next_event_sequence
                 )
                 VALUES (?1, ?2, 0, NULL, 0)",
                params![session_id.to_string(), agent_id.to_string()],
            )
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)
    }

    /// Durably admits an exact command and returns its stable operation.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the command identity is reused with different
    /// content or for another session.
    pub fn submit(
        &self,
        session_id: SessionId,
        command: Command,
    ) -> Result<Admission, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let content_json = serde_json::to_string(&command).map_err(json_error)?;
        let command_id = command.into_command_id();
        let existing = transaction
            .query_row(
                "SELECT o.operation_id, o.position, c.session_id, c.content_json
                 FROM commands AS c
                 JOIN operations AS o ON o.command_id = c.command_id
                 WHERE c.command_id = ?1",
                [command_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error)?;
        if let Some((operation_id, position, stored_session, stored_content)) = existing {
            let operation_id = parse_operation_id(&operation_id)?;
            if stored_session != session_id.to_string() || stored_content != content_json {
                return Err(KernelError::CommandConflict {
                    command_id,
                    operation_id,
                });
            }
            return Ok(Admission {
                operation_id,
                position: from_sql_integer(position, "operation position")?,
            });
        }

        let position = transaction
            .query_row(
                "SELECT next_command_position FROM sessions WHERE session_id = ?1",
                [session_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or(KernelError::SessionNotFound(session_id))?;
        let operation_id = OperationId::new();
        transaction
            .execute(
                "INSERT INTO commands (command_id, session_id, content_json)
                 VALUES (?1, ?2, ?3)",
                params![command_id.to_string(), session_id.to_string(), content_json],
            )
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO operations (
                    operation_id, session_id, command_id, position, phase,
                    state_version, transition_version, manifest_json, checkpoint_json,
                    current_effect_id, input_effect_id, outcome_json, next_effect_position
                 ) VALUES (
                    ?1, ?2, ?3, ?4, 'queued', ?5, 0,
                    NULL, NULL, NULL, NULL, NULL, 0
                 )",
                params![
                    operation_id.to_string(),
                    session_id.to_string(),
                    command_id.to_string(),
                    position,
                    crate::schema::OPERATION_STATE_VERSION,
                ],
            )
            .map_err(sqlite_error)?;
        let changed = transaction
            .execute(
                "UPDATE sessions
                 SET next_command_position = next_command_position + 1
                 WHERE session_id = ?1",
                [session_id.to_string()],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "session admission cursor update failed".to_owned(),
            ));
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(Admission {
            operation_id,
            position: from_sql_integer(position, "operation position")?,
        })
    }
}

pub(crate) fn parse_agent_id(value: &str) -> Result<AgentId, KernelError> {
    uuid::Uuid::parse_str(value)
        .map(AgentId::from_uuid)
        .map_err(|error| KernelError::Corrupt(format!("invalid agent id: {error}")))
}

pub(crate) fn parse_command_id(value: &str) -> Result<crate::CommandId, KernelError> {
    uuid::Uuid::parse_str(value)
        .map(crate::CommandId::from_uuid)
        .map_err(|error| KernelError::Corrupt(format!("invalid command id: {error}")))
}

pub(crate) fn parse_operation_id(value: &str) -> Result<OperationId, KernelError> {
    uuid::Uuid::parse_str(value)
        .map(OperationId::from_uuid)
        .map_err(|error| KernelError::Corrupt(format!("invalid operation id: {error}")))
}

pub(crate) fn from_sql_integer(value: i64, description: &str) -> Result<u64, KernelError> {
    u64::try_from(value)
        .map_err(|error| KernelError::Corrupt(format!("invalid {description} `{value}`: {error}")))
}
