use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    EventId, Kernel, KernelError, NewEvent, OperationId, SessionId, admission::from_sql_integer,
};
use crate::{admission::parse_operation_id, schema::sqlite_error};

/// The next unread session-local semantic event sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventCursor(u64);

impl EventCursor {
    pub const START: Self = Self(0);

    #[must_use]
    pub const fn new(next_sequence: u64) -> Self {
        Self(next_sequence)
    }

    #[must_use]
    pub const fn next_sequence(self) -> u64 {
        self.0
    }
}

/// One portable semantic session fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticEvent {
    pub event_id: EventId,
    pub operation_id: OperationId,
    pub sequence: u64,
    pub kind: String,
    pub payload: Value,
}

/// A gapless page through one captured durable high-water mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPage {
    pub events: Vec<SemanticEvent>,
    pub next_cursor: EventCursor,
}

pub(crate) fn validate_new_events(events: &[NewEvent]) -> Result<(), KernelError> {
    if events.iter().any(|event| event.kind.is_empty()) {
        Err(KernelError::InvalidDecision(
            "semantic event kind cannot be empty".to_owned(),
        ))
    } else {
        Ok(())
    }
}

impl Kernel {
    /// Reads semantic events at or after the supplied next-unread cursor.
    ///
    /// # Errors
    ///
    /// Rejects a cursor beyond the durable high-water mark.
    pub fn events_after(
        &self,
        session_id: SessionId,
        cursor: EventCursor,
    ) -> Result<EventPage, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
            .map_err(sqlite_error)?;
        let page = load_event_page(&transaction, session_id, cursor)?;
        transaction.commit().map_err(sqlite_error)?;
        Ok(page)
    }
}

pub(crate) fn load_event_page(
    connection: &rusqlite::Connection,
    session_id: SessionId,
    cursor: EventCursor,
) -> Result<EventPage, KernelError> {
    let high_water = connection
        .query_row(
            "SELECT next_event_sequence FROM sessions WHERE session_id = ?1",
            [session_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => KernelError::SessionNotFound(session_id),
            other => sqlite_error(other),
        })?;
    let high_water = from_sql_integer(high_water, "event high-water mark")?;
    if cursor.next_sequence() > high_water {
        return Err(KernelError::CursorAhead {
            cursor: cursor.next_sequence(),
            high_water,
        });
    }
    let start = i64::try_from(cursor.next_sequence())
        .map_err(|error| KernelError::Corrupt(format!("event cursor exceeds i64: {error}")))?;
    let events = load_events(connection, session_id, start)?;
    validate_gapless(cursor.next_sequence(), high_water, &events)?;
    Ok(EventPage {
        events,
        next_cursor: EventCursor::new(high_water),
    })
}

fn validate_gapless(
    start: u64,
    high_water: u64,
    events: &[SemanticEvent],
) -> Result<(), KernelError> {
    let mut expected = start;
    for event in events {
        if event.sequence != expected {
            return Err(KernelError::Corrupt(format!(
                "semantic event sequence {} was expected to be {expected}",
                event.sequence
            )));
        }
        expected = expected
            .checked_add(1)
            .ok_or_else(|| KernelError::Corrupt("event sequence overflowed".to_owned()))?;
    }
    if expected == high_water {
        Ok(())
    } else {
        Err(KernelError::Corrupt(format!(
            "semantic event replay ended at {expected}, below high-water mark {high_water}"
        )))
    }
}

pub(crate) fn load_events(
    connection: &rusqlite::Connection,
    session_id: SessionId,
    start: i64,
) -> Result<Vec<SemanticEvent>, KernelError> {
    let mut statement = connection
        .prepare(
            "SELECT event_id, operation_id, sequence, kind, payload_json
             FROM semantic_events
             WHERE session_id = ?1 AND sequence >= ?2
             ORDER BY sequence",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(rusqlite::params![session_id.to_string(), start], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(sqlite_error)?;
    let mut events = Vec::new();
    for row in rows {
        let (event_id, operation_id, sequence, kind, payload) = row.map_err(sqlite_error)?;
        events.push(SemanticEvent {
            event_id: parse_event_id(&event_id)?,
            operation_id: parse_operation_id(&operation_id)?,
            sequence: from_sql_integer(sequence, "event sequence")?,
            kind,
            payload: serde_json::from_str(&payload).map_err(crate::schema::json_error)?,
        });
    }
    Ok(events)
}

fn parse_event_id(value: &str) -> Result<EventId, KernelError> {
    uuid::Uuid::parse_str(value)
        .map(EventId::from_uuid)
        .map_err(|error| KernelError::Corrupt(format!("invalid event id: {error}")))
}
