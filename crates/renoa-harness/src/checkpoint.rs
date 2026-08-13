use renoa_agent::Message;
use rusqlite::{OptionalExtension, Transaction, params};
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, SessionId,
    schema::{json_error, sqlite_error},
};

const CHECKPOINT_PREFIX: &str = "[CONTEXT CHECKPOINT]\n";
const CHECKPOINT_SUFFIX: &str = "\n[END CONTEXT CHECKPOINT]";

#[derive(Clone)]
pub(crate) struct ContextEntry {
    pub(crate) operation_id: OperationId,
    pub(crate) sequence: u64,
    pub(crate) message: Message,
}

#[derive(Clone)]
pub(crate) struct ActiveCheckpoint {
    pub(crate) checkpoint_id: Uuid,
    pub(crate) covered_through_sequence: u64,
    pub(crate) summary: String,
}

pub(crate) fn load_context_view(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    active_operation_id: OperationId,
) -> Result<Vec<Message>, HarnessError> {
    let checkpoint = load_active_checkpoint(transaction, session_id)?;
    let Some(checkpoint) = checkpoint else {
        return load_entries_after(transaction, session_id, None)
            .map(|entries| entries.into_iter().map(|entry| entry.message).collect());
    };
    let mut messages = vec![checkpoint_message(&checkpoint.summary)];
    if let Some(anchor) = load_active_user_anchor(
        transaction,
        session_id,
        active_operation_id,
        checkpoint.covered_through_sequence,
    )? {
        messages.push(anchor);
    }
    messages.extend(
        load_entries_after(
            transaction,
            session_id,
            Some(checkpoint.covered_through_sequence),
        )?
        .into_iter()
        .map(|entry| entry.message),
    );
    Ok(messages)
}

pub(crate) fn load_active_checkpoint(
    transaction: &Transaction<'_>,
    session_id: SessionId,
) -> Result<Option<ActiveCheckpoint>, HarnessError> {
    let checkpoint_id = transaction
        .query_row(
            "SELECT active_checkpoint_id FROM sessions WHERE session_id = ?1",
            [session_id.to_string()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or(HarnessError::SessionNotFound(session_id))?;
    checkpoint_id
        .map(|checkpoint_id| {
            transaction
                .query_row(
                    "SELECT previous_checkpoint_id, covered_through_sequence, summary
                     FROM context_checkpoints
                     WHERE checkpoint_id = ?1 AND session_id = ?2",
                    params![checkpoint_id, session_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| {
                    HarnessError::Corrupt("active context checkpoint is missing".to_owned())
                })
                .and_then(|(previous, sequence, summary)| {
                    validate_checkpoint_parent(
                        transaction,
                        session_id,
                        previous.as_deref(),
                        sequence,
                    )?;
                    Ok(ActiveCheckpoint {
                        checkpoint_id: parse_uuid(&checkpoint_id, "checkpoint id")?,
                        covered_through_sequence: u64::try_from(sequence).map_err(|_| {
                            HarnessError::Corrupt(
                                "checkpoint has a negative transcript sequence".to_owned(),
                            )
                        })?,
                        summary,
                    })
                })
        })
        .transpose()
}

fn validate_checkpoint_parent(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    previous_checkpoint_id: Option<&str>,
    covered_through_sequence: i64,
) -> Result<(), HarnessError> {
    let Some(previous_checkpoint_id) = previous_checkpoint_id else {
        return Ok(());
    };
    let previous_sequence = transaction
        .query_row(
            "SELECT covered_through_sequence FROM context_checkpoints
             WHERE checkpoint_id = ?1 AND session_id = ?2",
            params![previous_checkpoint_id, session_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| {
            HarnessError::Corrupt(
                "active context checkpoint has an invalid previous checkpoint".to_owned(),
            )
        })?;
    if previous_sequence >= covered_through_sequence {
        return Err(HarnessError::Corrupt(
            "context checkpoint chain does not advance its transcript boundary".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn load_entries_after(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    covered_through: Option<u64>,
) -> Result<Vec<ContextEntry>, HarnessError> {
    let minimum = match covered_through {
        Some(sequence) => i64::try_from(sequence)
            .map_err(|_| HarnessError::Corrupt("checkpoint sequence exceeds i64".to_owned()))?,
        None => -1,
    };
    let mut statement = transaction
        .prepare(
            "SELECT operation_id, sequence, message_json
             FROM conversation_entries
             WHERE session_id = ?1 AND sequence > ?2
             ORDER BY sequence",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(params![session_id.to_string(), minimum], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(sqlite_error)?;
    let mut entries = Vec::new();
    for row in rows {
        let (operation_id, sequence, message_json) = row.map_err(sqlite_error)?;
        entries.push(ContextEntry {
            operation_id: operation_id
                .parse()
                .map(OperationId::from_uuid)
                .map_err(|error| HarnessError::Corrupt(format!("invalid operation id: {error}")))?,
            sequence: u64::try_from(sequence).map_err(|_| {
                HarnessError::Corrupt("conversation entry has a negative sequence".to_owned())
            })?,
            message: serde_json::from_str(&message_json).map_err(json_error)?,
        });
    }
    Ok(entries)
}

pub(crate) fn checkpoint_message(summary: &str) -> Message {
    Message::user_text(format!("{CHECKPOINT_PREFIX}{summary}{CHECKPOINT_SUFFIX}"))
}

fn load_active_user_anchor(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
    covered_through: u64,
) -> Result<Option<Message>, HarnessError> {
    let anchor = load_operation_user_anchor(transaction, session_id, operation_id)?;
    if anchor.sequence > covered_through {
        return Ok(None);
    }
    Ok(Some(anchor.message))
}

pub(crate) fn load_operation_user_anchor(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
) -> Result<ContextEntry, HarnessError> {
    let row = transaction
        .query_row(
            "SELECT sequence, message_json FROM conversation_entries
             WHERE session_id = ?1 AND operation_id = ?2 ORDER BY sequence LIMIT 1",
            params![session_id.to_string(), operation_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("active operation has no user entry".to_owned()))?;
    let sequence = u64::try_from(row.0)
        .map_err(|_| HarnessError::Corrupt("user entry has a negative sequence".to_owned()))?;
    let message: Message = serde_json::from_str(&row.1).map_err(json_error)?;
    if !matches!(message, Message::User { .. }) {
        return Err(HarnessError::Corrupt(
            "active operation does not start with a user message".to_owned(),
        ));
    }
    Ok(ContextEntry {
        operation_id,
        sequence,
        message,
    })
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, HarnessError> {
    value
        .parse()
        .map_err(|error| HarnessError::Corrupt(format!("invalid {field}: {error}")))
}
