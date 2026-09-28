//! Durable events recorded while a turn runs.
//!
//! The end-of-turn history projection repeats what was already recorded live,
//! and a restarted node re-drives the same turn. Each event is therefore
//! identified by what it reports: a tool start or finish by its call id, and an
//! assistant message by its text. An identified event is recorded once per
//! occurrence, whichever path reports it first.

use std::{collections::HashMap, sync::Arc};

use renoa_protocol::{CommandId, ExecutionEvent, ExecutionEventKind};
use rusqlite::{Transaction, TransactionBehavior};

use super::{
    NodeStore, NodeStoreError, blocking,
    records::{insert_event, load_record, next_event_sequence},
    schema::open_connection,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum EventKey {
    ToolStarted(String),
    ToolFinished(String),
    Message(String),
    Other(String),
}

impl EventKey {
    fn of(kind: &ExecutionEventKind) -> Result<Self, NodeStoreError> {
        Ok(match kind {
            ExecutionEventKind::ToolStarted { call_id, .. } => Self::ToolStarted(call_id.clone()),
            ExecutionEventKind::ToolFinished { call_id, .. } => Self::ToolFinished(call_id.clone()),
            ExecutionEventKind::AssistantMessage { text } => Self::Message(text.clone()),
            other => Self::Other(serde_json::to_string(other)?),
        })
    }
}

/// How many times each identified event is already recorded for a command.
pub(super) struct Recorded(HashMap<EventKey, usize>);

impl Recorded {
    pub(super) fn load(
        transaction: &Transaction<'_>,
        command_id: CommandId,
    ) -> Result<Self, NodeStoreError> {
        let mut statement =
            transaction.prepare("SELECT event_json FROM host_node_events WHERE command_id = ?1")?;
        let rows = statement.query_map([command_id.to_string()], |row| row.get::<_, String>(0))?;
        let mut counts = HashMap::new();
        for row in rows {
            let event: ExecutionEvent = serde_json::from_str(&row?)?;
            *counts.entry(EventKey::of(&event.kind)?).or_insert(0) += 1;
        }
        Ok(Self(counts))
    }

    /// The projected events not yet recorded, in projection order. Each
    /// recorded occurrence absorbs one projected occurrence.
    pub(super) fn unrecorded(
        mut self,
        kinds: Vec<ExecutionEventKind>,
    ) -> Result<Vec<ExecutionEventKind>, NodeStoreError> {
        let mut remaining = Vec::with_capacity(kinds.len());
        for kind in kinds {
            match self.0.get_mut(&EventKey::of(&kind)?) {
                Some(count) if *count > 0 => *count -= 1,
                _ => remaining.push(kind),
            }
        }
        Ok(remaining)
    }

    fn contains(&self, kind: &ExecutionEventKind) -> Result<bool, NodeStoreError> {
        Ok(self.0.contains_key(&EventKey::of(kind)?))
    }
}

impl NodeStore {
    /// Records one durable event of a running execution. Returns whether it
    /// was recorded: an event already recorded for the command, or one that
    /// arrives after the execution finished, is skipped.
    pub(crate) async fn append_progress(
        &self,
        command_id: CommandId,
        kind: ExecutionEventKind,
    ) -> Result<bool, NodeStoreError> {
        if matches!(
            kind,
            ExecutionEventKind::ExecutionStarted
                | ExecutionEventKind::TurnStarted
                | ExecutionEventKind::ExecutionTerminated { .. }
        ) {
            return Err(NodeStoreError::Invalid(
                "live progress carries only assistant messages and tool events".to_owned(),
            ));
        }
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let record = load_record(&transaction, command_id)?.ok_or_else(|| {
                NodeStoreError::Invalid(format!("execution for command {command_id} was not found"))
            })?;
            if record.terminal || Recorded::load(&transaction, command_id)?.contains(&kind)? {
                transaction.commit()?;
                return Ok(false);
            }
            let sequence = next_event_sequence(&transaction, command_id)?;
            insert_event(
                &transaction,
                command_id,
                record.execution_id,
                sequence,
                kind,
            )?;
            transaction.commit()?;
            Ok(true)
        })
        .await
    }
}
