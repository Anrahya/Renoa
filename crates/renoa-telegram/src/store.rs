use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::ingress::{ParsedUpdate, Topic};
use rusqlite::{Connection, OptionalExtension as _, params};

mod actions;
mod admission;
mod schema;
#[cfg(test)]
mod tests;
mod types;
mod util;

pub(crate) use types::{
    Admission, DeliveryItem, ImmediateAction, PendingAction, RecoveryReport, StoreError, WorkItem,
    WorkKind,
};
use util::{encoded_path, now_ms, parse_uuid, require_one, require_payload, restrict_directory};

#[derive(Clone)]
pub(crate) struct SurfaceStore {
    database: Arc<PathBuf>,
    _lease: Arc<std::fs::File>,
}

impl SurfaceStore {
    pub(crate) fn open(data_directory: &Path) -> Result<Self, StoreError> {
        let surface_directory = data_directory.join("surfaces").join("telegram");
        std::fs::create_dir_all(&surface_directory)?;
        restrict_directory(&surface_directory)?;
        let lease = schema::acquire_lease(&surface_directory)?;
        let database = surface_directory.join(schema::DATABASE_FILE);
        drop(schema::open(&database)?);
        schema::restrict_database(&database)?;
        Ok(Self {
            database: Arc::new(database),
            _lease: Arc::new(lease),
        })
    }

    pub(crate) async fn bind_identity(
        &self,
        bot_id: i64,
        allowed_user_id: i64,
        workspace: &Path,
    ) -> Result<(), StoreError> {
        let workspace = encoded_path(workspace);
        self.access(move |connection| {
            let existing = connection
                .query_row(
                    "SELECT bot_id, allowed_user_id, workspace FROM surface_identity WHERE singleton = 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                        ))
                    },
                )
                .optional()?;
            match existing {
                Some(existing) if existing == (bot_id, allowed_user_id, workspace.clone()) => Ok(()),
                Some(_) => Err(StoreError::Invalid(
                    "stored Telegram bot, owner, or workspace differs from this process configuration"
                        .to_owned(),
                )),
                None => {
                    connection.execute(
                        "INSERT INTO surface_identity(
                            singleton, bot_id, allowed_user_id, workspace, next_update_id
                         ) VALUES (1, ?1, ?2, ?3, 0)",
                        params![bot_id, allowed_user_id, workspace],
                    )?;
                    Ok(())
                }
            }
        })
        .await
    }

    pub(crate) async fn recover(&self) -> Result<RecoveryReport, StoreError> {
        self.access(|connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let now = now_ms()?;
            let requeued = transaction.execute(
                "UPDATE updates SET state = 'queued', updated_at_ms = ?1
                 WHERE state = 'running'",
                [now],
            )?;
            let delivery_unknown = transaction.execute(
                "UPDATE updates
                 SET state = 'delivery_unknown',
                     error = 'process ended while Telegram final delivery was in flight',
                     updated_at_ms = ?1
                 WHERE state = 'delivering'",
                [now],
            )?;
            let action_delivery_unknown = transaction.execute(
                "UPDATE surface_actions
                 SET state = 'delivery_unknown',
                     error = 'process ended while Telegram action delivery was in flight',
                     updated_at_ms = ?1
                 WHERE state = 'delivering'",
                [now],
            )?;
            actions::scrub_settled_action_urls(&transaction, now)?;
            transaction.commit()?;
            Ok(RecoveryReport {
                requeued,
                delivery_unknown,
                action_delivery_unknown,
            })
        })
        .await
    }

    pub(crate) async fn next_update_id(&self) -> Result<i64, StoreError> {
        self.access(|connection| {
            connection
                .query_row(
                    "SELECT next_update_id FROM surface_identity WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
        })
        .await
    }

    pub(crate) async fn admit(&self, parsed: ParsedUpdate) -> Result<Admission, StoreError> {
        self.access(move |connection| admission::admit(connection, &parsed))
            .await
    }

    pub(crate) async fn next_action(&self) -> Result<Option<PendingAction>, StoreError> {
        self.access(load_next_action).await
    }

    pub(crate) async fn mark_running(&self, update_id: i64) -> Result<(), StoreError> {
        self.transition(update_id, "queued", "running", None).await
    }

    pub(crate) async fn cancellation_requested(&self, update_id: i64) -> Result<bool, StoreError> {
        self.access(move |connection| {
            connection
                .query_row(
                    "SELECT cancel_requested FROM updates WHERE update_id = ?1",
                    [update_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(StoreError::from)
        })
        .await
    }

    pub(crate) async fn set_result(&self, update_id: i64, text: String) -> Result<(), StoreError> {
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let now = now_ms()?;
            let changed = transaction.execute(
                "UPDATE updates
                 SET state = 'ready', result = ?1, error = NULL, updated_at_ms = ?2
                 WHERE update_id = ?3 AND state = 'running'",
                params![text, now, update_id],
            )?;
            require_one(changed, update_id, "running", "ready")?;
            actions::scrub_action_urls_for_update(&transaction, update_id, now)?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn mark_delivering(&self, update_id: i64) -> Result<(), StoreError> {
        self.transition(update_id, "ready", "delivering", None)
            .await
    }

    pub(crate) async fn mark_chunk_delivered(
        &self,
        update_id: i64,
        cursor: usize,
        telegram_message_id: i64,
        complete: bool,
    ) -> Result<(), StoreError> {
        let cursor = i64::try_from(cursor).map_err(|_| {
            StoreError::Invalid("delivery cursor exceeded SQLite integer".to_owned())
        })?;
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let stored = transaction
                .query_row(
                    "SELECT delivery_cursor FROM updates
                     WHERE update_id = ?1 AND state = 'delivering'",
                    [update_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            if stored != Some(cursor) {
                return Err(StoreError::Invalid(format!(
                    "update {update_id} delivery cursor changed while sending"
                )));
            }
            transaction.execute(
                "INSERT INTO delivery_messages(
                    update_id, chunk_index, telegram_message_id, delivered_at_ms
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![update_id, cursor, telegram_message_id, now_ms()?],
            )?;
            transaction.execute(
                "UPDATE updates
                 SET state = ?1, delivery_cursor = ?2, updated_at_ms = ?3
                 WHERE update_id = ?4 AND state = 'delivering'",
                params![
                    if complete { "delivered" } else { "ready" },
                    cursor + 1,
                    now_ms()?,
                    update_id
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn defer_delivery(&self, update_id: i64) -> Result<(), StoreError> {
        self.transition(update_id, "delivering", "ready", None)
            .await
    }

    pub(crate) async fn mark_delivery_unknown(
        &self,
        update_id: i64,
        error: String,
    ) -> Result<(), StoreError> {
        self.transition(update_id, "delivering", "delivery_unknown", Some(error))
            .await
    }

    pub(crate) async fn mark_delivery_failed(
        &self,
        update_id: i64,
        error: String,
    ) -> Result<(), StoreError> {
        self.transition(update_id, "delivering", "delivery_failed", Some(error))
            .await
    }

    async fn transition(
        &self,
        update_id: i64,
        from: &'static str,
        to: &'static str,
        error: Option<String>,
    ) -> Result<(), StoreError> {
        self.access(move |connection| {
            let changed = connection.execute(
                "UPDATE updates SET state = ?1, error = ?2, updated_at_ms = ?3
                 WHERE update_id = ?4 AND state = ?5",
                params![to, error, now_ms()?, update_id, from],
            )?;
            require_one(changed, update_id, from, to)
        })
        .await
    }

    async fn access<T, F>(&self, action: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let database = Arc::clone(&self.database);
        tokio::task::spawn_blocking(move || {
            let mut connection = schema::open(&database)?;
            action(&mut connection)
        })
        .await?
    }
}

fn load_next_action(connection: &mut Connection) -> Result<Option<PendingAction>, StoreError> {
    let row = connection
        .query_row(
            "SELECT update_id, chat_id, thread_id, session_id, request_id,
                    draft_id, kind, payload, state, result, delivery_cursor,
                    created_at_ms
             FROM updates
             WHERE state IN ('queued', 'ready')
             ORDER BY update_id
             LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            },
        )
        .optional()?;
    let Some((
        update_id,
        chat_id,
        thread_id,
        session,
        request,
        draft_id,
        kind,
        payload,
        state,
        result,
        cursor,
        observed_at_ms,
    )) = row
    else {
        return Ok(None);
    };
    let topic = Topic {
        chat_id,
        thread_id: (thread_id != 0).then_some(thread_id),
    };
    if state == "ready" {
        return Ok(Some(PendingAction::Deliver(DeliveryItem {
            update_id,
            topic,
            text: result.ok_or_else(|| {
                StoreError::Invalid(format!("ready update {update_id} has no result"))
            })?,
            cursor: usize::try_from(cursor).map_err(|_| {
                StoreError::Invalid(format!("update {update_id} has an invalid delivery cursor"))
            })?,
        })));
    }
    Ok(Some(PendingAction::Execute(WorkItem {
        update_id,
        topic,
        session_id: parse_uuid(&session, "session")?,
        request_id: parse_uuid(&request, "request")?,
        draft_id,
        observed_at_ms,
        kind: decode_work(&kind, payload)?,
    })))
}

fn decode_work(kind: &str, payload: Option<String>) -> Result<WorkKind, StoreError> {
    match kind {
        "prompt" => Ok(WorkKind::Prompt(require_payload(kind, payload)?)),
        "compact" => Ok(WorkKind::Compact),
        "new" => Ok(WorkKind::New),
        "status" => Ok(WorkKind::Status),
        "model" => Ok(WorkKind::Model(payload)),
        "reasoning" => Ok(WorkKind::Reasoning(payload)),
        "cancel" => Ok(WorkKind::Cancel),
        "notice" => Ok(WorkKind::Notice(require_payload(kind, payload)?)),
        _ => Err(StoreError::Invalid(format!(
            "queued update has unsupported kind {kind}"
        ))),
    }
}
