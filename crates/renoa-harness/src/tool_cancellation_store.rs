use renoa_agent::{ContentBlock, Message, ToolCall, ToolResult};
use rusqlite::params;
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, SessionId,
    drive::ToolSettlement,
    schema::{json_error, sqlite_error},
    state::ToolBatch,
    store_support::{finish_cancelled_operation, load_cursors},
    tool_store::ToolResultCommit,
};

pub(crate) fn cancelled_before_execution_result(call: &ToolCall) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text(
            "Tool call was not executed because the operation was cancelled.",
        )],
        details: None,
        is_error: true,
    }
}

pub(crate) fn cancel_tool_batch(
    transaction: &rusqlite::Transaction<'_>,
    commit: ToolResultCommit<'_>,
) -> Result<ToolSettlement, HarnessError> {
    let (entry_sequence, _) = load_cursors(transaction, commit.session_id)?;
    insert_tool_message(
        transaction,
        &commit.result_entry_id.to_string(),
        commit.session_id,
        commit.operation_id,
        entry_sequence,
        commit.result,
    )?;
    let remaining = load_remaining_planned_calls(transaction, commit.operation_id, commit.batch)?;
    for (offset, call) in remaining.iter().enumerate() {
        let offset = i64::try_from(offset)
            .map_err(|_| HarnessError::Corrupt("tool-result offset exceeds i64".to_owned()))?;
        let sequence = entry_sequence
            .checked_add(1)
            .and_then(|value| value.checked_add(offset))
            .ok_or_else(|| HarnessError::Corrupt("entry cursor overflowed".to_owned()))?;
        insert_tool_message(
            transaction,
            &call.result_entry_id,
            commit.session_id,
            commit.operation_id,
            sequence,
            cancelled_before_execution_result(&call.call),
        )?;
    }
    let deleted = transaction
        .execute(
            "DELETE FROM tool_calls WHERE operation_id = ?1 AND batch_id = ?2",
            params![
                commit.operation_id.to_string(),
                commit.batch.batch_id.to_string(),
            ],
        )
        .map_err(sqlite_error)?;
    if deleted != remaining.len() {
        return Err(HarnessError::Corrupt(
            "cancelled tool batch changed before settlement".to_owned(),
        ));
    }
    let inserted_entries = i64::try_from(remaining.len())
        .ok()
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| HarnessError::Corrupt("tool-result count exceeds i64".to_owned()))?;
    let outcome = finish_cancelled_operation(
        transaction,
        commit.session_id,
        commit.operation_id,
        Uuid::new_v4(),
        commit.old_state_json,
        inserted_entries,
    )?;
    Ok(ToolSettlement::Finished(outcome))
}

struct RemainingPlannedCall {
    result_entry_id: String,
    call: ToolCall,
}

fn load_remaining_planned_calls(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
    batch: ToolBatch,
) -> Result<Vec<RemainingPlannedCall>, HarnessError> {
    let first_index = batch
        .next_index
        .checked_add(1)
        .ok_or_else(|| HarnessError::Corrupt("tool-call index overflowed".to_owned()))?;
    let mut statement = transaction
        .prepare(
            "SELECT source_index, result_entry_id, call_json, status
             FROM tool_calls WHERE operation_id = ?1 AND batch_id = ?2
             ORDER BY source_index",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(
            params![operation_id.to_string(), batch.batch_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(sqlite_error)?;
    let mut calls = Vec::new();
    for (offset, row) in rows.enumerate() {
        let (source_index, result_entry_id, call_json, status) = row.map_err(sqlite_error)?;
        let expected_index =
            i64::from(first_index)
                .checked_add(i64::try_from(offset).map_err(|_| {
                    HarnessError::Corrupt("tool-call offset exceeds i64".to_owned())
                })?)
                .ok_or_else(|| HarnessError::Corrupt("tool-call index overflowed".to_owned()))?;
        if source_index != expected_index || status != "planned" {
            return Err(HarnessError::Corrupt(
                "cancelled tool batch is incomplete or out of order".to_owned(),
            ));
        }
        calls.push(RemainingPlannedCall {
            result_entry_id,
            call: serde_json::from_str(&call_json).map_err(json_error)?,
        });
    }
    let expected = batch
        .call_count
        .checked_sub(first_index)
        .ok_or_else(|| HarnessError::Corrupt("tool batch cursor exceeds its count".to_owned()))?;
    if calls.len()
        != usize::try_from(expected)
            .map_err(|_| HarnessError::Corrupt("tool-call count exceeds usize".to_owned()))?
    {
        return Err(HarnessError::Corrupt(
            "cancelled tool batch has missing calls".to_owned(),
        ));
    }
    Ok(calls)
}

fn insert_tool_message(
    transaction: &rusqlite::Transaction<'_>,
    entry_id: &str,
    session_id: SessionId,
    operation_id: OperationId,
    sequence: i64,
    result: ToolResult,
) -> Result<(), HarnessError> {
    transaction
        .execute(
            "INSERT INTO conversation_entries (
                entry_id, session_id, operation_id, sequence, message_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry_id,
                session_id.to_string(),
                operation_id.to_string(),
                sequence,
                serde_json::to_string(&Message::Tool { result }).map_err(json_error)?,
            ],
        )
        .map_err(sqlite_error)?;
    Ok(())
}
