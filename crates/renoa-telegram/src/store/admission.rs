use rusqlite::{Connection, OptionalExtension as _, params};
use uuid::Uuid;

use crate::ingress::{InboundKind, ParsedUpdate, Topic};

use super::{
    Admission, ImmediateAction, StoreError, schema,
    util::{draft_id, now_ms},
};

pub(super) fn admit(
    connection: &mut Connection,
    parsed: &ParsedUpdate,
) -> Result<Admission, StoreError> {
    if parsed.update_id < 0 {
        return Err(StoreError::Invalid(
            "Telegram update identity cannot be negative".to_owned(),
        ));
    }
    let transaction = schema::immediate_transaction(connection)?;
    let existing = transaction
        .query_row(
            "SELECT canonical_json FROM updates WHERE update_id = ?1",
            [parsed.update_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing != parsed.canonical {
            return Err(StoreError::Invalid(format!(
                "Telegram reused update {} with different content",
                parsed.update_id
            )));
        }
        advance_offset(&transaction, parsed.update_id)?;
        transaction.commit()?;
        return Ok(Admission {
            duplicate: true,
            queued: false,
            immediate: None,
        });
    }

    let request_id = Uuid::new_v4();
    let draft_id = draft_id(request_id);
    let session_id = match (parsed.topic, &parsed.kind) {
        (Some(topic), InboundKind::New) => Some(replace_conversation(&transaction, topic)?),
        (Some(topic), kind) if kind.is_queued() => Some(require_conversation(&transaction, topic)?),
        _ => None,
    };
    if matches!(&parsed.kind, InboundKind::Cancel)
        && let Some(topic) = parsed.topic
    {
        request_cancellation(&transaction, topic)?;
    }
    let (payload, incoming_draft_id) = payload(&parsed.kind);
    let state = if parsed.kind.is_queued() {
        "queued"
    } else {
        "ignored"
    };
    let now = now_ms()?;
    transaction.execute(
        "INSERT INTO updates(
            update_id, canonical_json, chat_id, thread_id, message_id, session_id,
            request_id, draft_id, kind, payload, incoming_draft_id, state,
            created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        params![
            parsed.update_id,
            parsed.canonical,
            parsed.topic.map(|topic| topic.chat_id),
            parsed.topic.map(Topic::stored_thread_id),
            parsed.message_id,
            session_id.map(|id| id.to_string()),
            request_id.to_string(),
            draft_id,
            parsed.kind.storage_name(),
            payload,
            incoming_draft_id,
            state,
            now,
        ],
    )?;
    advance_offset(&transaction, parsed.update_id)?;
    transaction.commit()?;
    let immediate = match &parsed.kind {
        InboundKind::Cancel => parsed.topic.map(ImmediateAction::Cancel),
        InboundKind::Stopped { draft_id } => parsed.topic.map(|topic| ImmediateAction::Stop {
            topic,
            draft_id: *draft_id,
        }),
        _ => None,
    };
    Ok(Admission {
        duplicate: false,
        queued: parsed.kind.is_queued(),
        immediate,
    })
}

fn require_conversation(
    transaction: &rusqlite::Transaction<'_>,
    topic: Topic,
) -> Result<Uuid, StoreError> {
    if let Some(existing) = conversation(transaction, topic)? {
        return Ok(existing);
    }
    let session_id = Uuid::new_v4();
    insert_surface_session(transaction, topic, session_id)?;
    transaction.execute(
        "INSERT INTO conversations(chat_id, thread_id, session_id, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            topic.chat_id,
            topic.stored_thread_id(),
            session_id.to_string(),
            now_ms()?
        ],
    )?;
    Ok(session_id)
}

fn request_cancellation(
    transaction: &rusqlite::Transaction<'_>,
    topic: Topic,
) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE updates
         SET cancel_requested = 1, updated_at_ms = ?1
         WHERE update_id = (
            SELECT update_id FROM updates
            WHERE chat_id = ?2 AND thread_id = ?3
              AND kind IN ('prompt', 'compact')
              AND state IN ('running', 'queued')
            ORDER BY CASE state WHEN 'running' THEN 0 ELSE 1 END, update_id
            LIMIT 1
         )",
        params![now_ms()?, topic.chat_id, topic.stored_thread_id()],
    )?;
    Ok(())
}

fn replace_conversation(
    transaction: &rusqlite::Transaction<'_>,
    topic: Topic,
) -> Result<Uuid, StoreError> {
    let session_id = Uuid::new_v4();
    insert_surface_session(transaction, topic, session_id)?;
    transaction.execute(
        "INSERT INTO conversations(chat_id, thread_id, session_id, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(chat_id, thread_id) DO UPDATE SET
            session_id = excluded.session_id,
            updated_at_ms = excluded.updated_at_ms",
        params![
            topic.chat_id,
            topic.stored_thread_id(),
            session_id.to_string(),
            now_ms()?
        ],
    )?;
    Ok(session_id)
}

fn insert_surface_session(
    transaction: &rusqlite::Transaction<'_>,
    topic: Topic,
    session_id: Uuid,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO surface_sessions(session_id, chat_id, thread_id, created_at_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            session_id.to_string(),
            topic.chat_id,
            topic.stored_thread_id(),
            now_ms()?
        ],
    )?;
    Ok(())
}

fn conversation(
    transaction: &rusqlite::Transaction<'_>,
    topic: Topic,
) -> Result<Option<Uuid>, StoreError> {
    let value = transaction
        .query_row(
            "SELECT session_id FROM conversations WHERE chat_id = ?1 AND thread_id = ?2",
            params![topic.chat_id, topic.stored_thread_id()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    value.map_or(Ok(None), |value| {
        Uuid::parse_str(&value).map(Some).map_err(|_| {
            StoreError::Invalid("stored conversation session is not a UUID".to_owned())
        })
    })
}

fn advance_offset(
    transaction: &rusqlite::Transaction<'_>,
    update_id: i64,
) -> Result<(), StoreError> {
    let next = update_id
        .checked_add(1)
        .ok_or_else(|| StoreError::Invalid("Telegram update identity overflowed".to_owned()))?;
    let changed = transaction.execute(
        "UPDATE surface_identity
         SET next_update_id = max(next_update_id, ?1)
         WHERE singleton = 1",
        [next],
    )?;
    if changed != 1 {
        return Err(StoreError::Invalid(
            "Telegram surface identity has not been bound".to_owned(),
        ));
    }
    Ok(())
}

fn payload(kind: &InboundKind) -> (Option<&str>, Option<i64>) {
    match kind {
        InboundKind::Prompt(text) => (Some(text), None),
        InboundKind::Model(model) | InboundKind::Reasoning(model) => (model.as_deref(), None),
        InboundKind::Notice(text) | InboundKind::Ignored(text) => (Some(text), None),
        InboundKind::Stopped { draft_id } => (None, Some(*draft_id)),
        _ => (None, None),
    }
}
