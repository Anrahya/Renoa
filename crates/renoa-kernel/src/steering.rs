//! Messages that steer an unfinished operation.
//!
//! A steer is a command a caller asks to reach the operation of another
//! command (its target) while that operation runs, instead of starting one of
//! its own. It is recorded under its own command identity before the target
//! starts or while it runs, the loop accepts it at a step it chooses, and the
//! acceptance commits with the events that carry it. Its state is derived,
//! never written twice:
//!
//! - accepted: the loop took it into the target's operation;
//! - released: not accepted, and either the target's operation finished or
//!   the steer's own command was admitted as an operation;
//! - pending: neither.
//!
//! A released steer is an ordinary command again. An accepted steer can never
//! be admitted as an operation of its own, so a caller that cannot tell
//! whether it was accepted submits it and learns the answer.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    Command, CommandId, Kernel, KernelError, SessionId,
    admission::{UNFINISHED_OPERATION, parse_operation_id},
    schema::{json_error, sqlite_error},
};

/// The table of steers, shared by a new schema and the upgrade to schema 5.
pub(crate) const STEER_REQUESTS: &str = "CREATE TABLE steer_requests (
        command_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL REFERENCES sessions(session_id),
        target_command_id TEXT NOT NULL,
        content_json TEXT NOT NULL,
        accepted INTEGER NOT NULL CHECK (accepted IN (0, 1)),
        CHECK (command_id != target_command_id)
     ) STRICT;

     CREATE INDEX steer_requests_target
        ON steer_requests(session_id, target_command_id);";

/// Where one recorded steer stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerState {
    /// Waiting for the target's operation to accept it.
    Pending,
    /// Taken into the target's operation.
    Accepted,
    /// Never accepted; it is an ordinary command again.
    Released,
}

impl Kernel {
    /// Durably records `steer` for the operation of command `target` in
    /// `session_id`, before that operation starts or while it is unfinished.
    ///
    /// Repeating the same steer for the same target returns its current state.
    /// A refusal records nothing.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::SteerConflict`] when the steer's identity is
    /// recorded with another target or content,
    /// [`KernelError::CommandConflict`] when it is already an operation's
    /// command, and [`KernelError::SteerTargetUnavailable`] when the target's
    /// operation finished, belongs to another session, or is itself a steer.
    pub fn request_steer(
        &self,
        session_id: SessionId,
        target: CommandId,
        steer: Command,
    ) -> Result<SteerState, KernelError> {
        let content_json = serde_json::to_string(&steer).map_err(json_error)?;
        let steer_id = steer.into_command_id();
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let session_key = session_id.to_string();
        let target_key = target.to_string();
        if let Some((stored_session, stored_target, stored_content)) = transaction
            .query_row(
                "SELECT session_id, target_command_id, content_json
                 FROM steer_requests WHERE command_id = ?1",
                [steer_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error)?
        {
            if stored_session != session_key
                || stored_target != target_key
                || stored_content != content_json
            {
                return Err(KernelError::SteerConflict {
                    command_id: steer_id,
                });
            }
            let state = steer_state(&transaction, steer_id)?;
            transaction.commit().map_err(sqlite_error)?;
            return state
                .ok_or_else(|| KernelError::Corrupt("a recorded steer has no state".to_owned()));
        }
        if let Some(operation_id) = operation_of(&transaction, steer_id)? {
            return Err(KernelError::CommandConflict {
                command_id: steer_id,
                operation_id,
            });
        }
        let session_found: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = ?1)",
                [&session_key],
                |row| row.get(0),
            )
            .map_err(sqlite_error)?;
        if !session_found {
            return Err(KernelError::SessionNotFound(session_id));
        }
        let target_open: bool = transaction
            .query_row(
                &format!(
                    "SELECT NOT EXISTS(
                        SELECT 1 FROM steer_requests WHERE command_id = ?2
                     ) AND NOT EXISTS(
                        SELECT 1 FROM operations WHERE command_id = ?2
                          AND (session_id != ?1 OR NOT ({UNFINISHED_OPERATION}))
                     )"
                ),
                params![session_key, target_key],
                |row| row.get(0),
            )
            .map_err(sqlite_error)?;
        if !target_open {
            return Err(KernelError::SteerTargetUnavailable(target));
        }
        transaction
            .execute(
                "INSERT INTO steer_requests (
                    command_id, session_id, target_command_id, content_json, accepted
                 ) VALUES (?1, ?2, ?3, ?4, 0)",
                params![steer_id.to_string(), session_key, target_key, content_json],
            )
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
        Ok(SteerState::Pending)
    }
}

impl Kernel {
    /// Where steer `command_id` stands, or `None` when it was never recorded.
    ///
    /// # Errors
    ///
    /// Returns a storage failure.
    pub fn steer_state(&self, command_id: CommandId) -> Result<Option<SteerState>, KernelError> {
        steer_state(&self.database.connection()?, command_id)
    }
}

/// The derived state of steer `command_id`, if it was ever recorded.
pub(crate) fn steer_state(
    connection: &rusqlite::Connection,
    command_id: CommandId,
) -> Result<Option<SteerState>, KernelError> {
    connection
        .query_row(
            &format!(
                "SELECT s.accepted,
                    EXISTS(SELECT 1 FROM operations WHERE command_id = s.command_id)
                    OR EXISTS(
                        SELECT 1 FROM operations
                        WHERE command_id = s.target_command_id
                          AND NOT ({UNFINISHED_OPERATION})
                    )
                 FROM steer_requests AS s WHERE s.command_id = ?1"
            ),
            [command_id.to_string()],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)
        .map(|found| {
            found.map(|(accepted, released)| match (accepted, released) {
                (true, _) => SteerState::Accepted,
                (false, true) => SteerState::Released,
                (false, false) => SteerState::Pending,
            })
        })
}

/// The pending steers of the operation of `target`, in the order recorded.
/// The caller holds that operation active, so its target has not finished.
pub(crate) fn pending_steers(
    connection: &rusqlite::Connection,
    session_id: SessionId,
    target: CommandId,
) -> Result<Vec<Command>, KernelError> {
    let mut statement = connection
        .prepare(
            "SELECT s.content_json FROM steer_requests AS s
             WHERE s.session_id = ?1 AND s.target_command_id = ?2 AND s.accepted = 0
               AND NOT EXISTS(SELECT 1 FROM operations WHERE command_id = s.command_id)
             ORDER BY s.rowid",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(params![session_id.to_string(), target.to_string()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(sqlite_error)?;
    rows.map(|row| {
        let content = row.map_err(sqlite_error)?;
        serde_json::from_str(&content).map_err(json_error)
    })
    .collect()
}

/// Marks `steers` accepted by `operation_id`. Each must still be pending for
/// that operation's command, or the caller's transaction must not commit.
pub(crate) fn accept_steers(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: crate::OperationId,
    steers: &[CommandId],
) -> Result<(), KernelError> {
    if steers.is_empty() {
        return Err(KernelError::InvalidDecision(
            "accepting steers requires at least one steer".to_owned(),
        ));
    }
    let target = transaction
        .query_row(
            "SELECT command_id FROM operations WHERE operation_id = ?1",
            [operation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .map_err(sqlite_error)?;
    for steer in steers {
        let changed = transaction
            .execute(
                "UPDATE steer_requests SET accepted = 1
                 WHERE command_id = ?1 AND session_id = ?2 AND target_command_id = ?3
                   AND accepted = 0
                   AND NOT EXISTS(SELECT 1 FROM operations WHERE command_id = ?1)",
                params![steer.to_string(), session_id.to_string(), target],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::InvalidDecision(format!(
                "steer {steer} is not pending for this operation"
            )));
        }
    }
    Ok(())
}

/// Refuses to admit an accepted steer as an operation of its own.
pub(crate) fn require_not_accepted(
    connection: &rusqlite::Connection,
    command_id: CommandId,
) -> Result<(), KernelError> {
    let target = connection
        .query_row(
            "SELECT target_command_id FROM steer_requests
             WHERE command_id = ?1 AND accepted = 1",
            [command_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?;
    match target {
        None => Ok(()),
        Some(target) => Err(KernelError::SteerAccepted {
            command_id,
            target: uuid::Uuid::parse_str(&target)
                .map(CommandId::from_uuid)
                .map_err(|error| KernelError::Corrupt(format!("invalid steer target: {error}")))?,
        }),
    }
}

fn operation_of(
    connection: &rusqlite::Connection,
    command_id: CommandId,
) -> Result<Option<crate::OperationId>, KernelError> {
    connection
        .query_row(
            "SELECT operation_id FROM operations WHERE command_id = ?1",
            [command_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .map(|operation| parse_operation_id(&operation))
        .transpose()
}
