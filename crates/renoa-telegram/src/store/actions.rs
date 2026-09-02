use rusqlite::{OptionalExtension as _, params};

use super::{StoreError, SurfaceStore, now_ms, require_one, schema};
use crate::{actions::ActionLink, ingress::Topic};

pub(super) const SCRUBBED_ACTION_URL: &str = "https://redacted.invalid/";

pub(super) fn scrub_action_urls_for_update(
    transaction: &rusqlite::Transaction<'_>,
    update_id: i64,
    updated_at_ms: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE surface_actions SET url = ?1, updated_at_ms = ?2
         WHERE update_id = ?3 AND url != ?1",
        params![SCRUBBED_ACTION_URL, updated_at_ms, update_id],
    )?;
    Ok(())
}

pub(super) fn scrub_settled_action_urls(
    transaction: &rusqlite::Transaction<'_>,
    updated_at_ms: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE surface_actions SET url = ?1, updated_at_ms = ?2
         WHERE url != ?1 AND update_id IN (
             SELECT update_id FROM updates WHERE state NOT IN ('queued', 'running')
         )",
        params![SCRUBBED_ACTION_URL, updated_at_ms],
    )?;
    Ok(())
}

impl SurfaceStore {
    pub(crate) async fn begin_action_delivery(
        &self,
        update_id: i64,
        topic: Topic,
        action: ActionLink,
    ) -> Result<bool, StoreError> {
        validate_action(&action)?;
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let parent = transaction
                .query_row(
                    "SELECT chat_id, thread_id, state FROM updates WHERE update_id = ?1",
                    [update_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| StoreError::Invalid(format!("update {update_id} is missing")))?;
            if parent
                != (
                    Some(topic.chat_id),
                    Some(topic.stored_thread_id()),
                    "running".to_owned(),
                )
            {
                return Err(StoreError::Invalid(format!(
                    "action parent update {update_id} is not the active topic turn"
                )));
            }
            let now = now_ms()?;
            transaction.execute(
                "INSERT OR IGNORE INTO surface_actions(
                    action_id, update_id, kind, title, message, button, url,
                    expires_at_ms, state, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, 'open_url', ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?8)",
                params![
                    action.id,
                    update_id,
                    action.title,
                    action.message,
                    action.button,
                    action.url.as_str(),
                    action.expires_at_ms,
                    now,
                ],
            )?;
            let stored = transaction.query_row(
                "SELECT update_id, title, message, button, url, expires_at_ms, state
                 FROM surface_actions WHERE action_id = ?1",
                [&action.id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )?;
            if stored.0 != update_id
                || stored.1 != action.title
                || stored.2 != action.message
                || stored.3 != action.button
                || stored.4 != action.url.as_str()
                || stored.5 != action.expires_at_ms
            {
                return Err(StoreError::Invalid(format!(
                    "surface action '{}' was reused with different content",
                    action.id
                )));
            }
            let should_send = stored.6 == "pending";
            if should_send {
                let changed = transaction.execute(
                    "UPDATE surface_actions SET state = 'delivering', updated_at_ms = ?1
                     WHERE action_id = ?2 AND state = 'pending'",
                    params![now_ms()?, action.id],
                )?;
                require_one(changed, update_id, "pending action", "delivering action")?;
            }
            transaction.commit()?;
            Ok(should_send)
        })
        .await
    }

    pub(crate) async fn mark_action_delivered(
        &self,
        action_id: &str,
        telegram_message_id: i64,
    ) -> Result<(), StoreError> {
        self.action_transition(
            action_id,
            "delivering",
            "delivered",
            Some(telegram_message_id),
            None,
        )
        .await
    }

    pub(crate) async fn defer_action_delivery(&self, action_id: &str) -> Result<(), StoreError> {
        self.action_transition(action_id, "delivering", "pending", None, None)
            .await
    }

    pub(crate) async fn mark_action_delivery_unknown(
        &self,
        action_id: &str,
        error: String,
    ) -> Result<(), StoreError> {
        self.action_transition(
            action_id,
            "delivering",
            "delivery_unknown",
            None,
            Some(error),
        )
        .await
    }

    pub(crate) async fn mark_action_delivery_failed(
        &self,
        action_id: &str,
        error: String,
    ) -> Result<(), StoreError> {
        self.action_transition(
            action_id,
            "delivering",
            "delivery_failed",
            None,
            Some(error),
        )
        .await
    }

    async fn action_transition(
        &self,
        action_id: &str,
        from: &'static str,
        to: &'static str,
        telegram_message_id: Option<i64>,
        error: Option<String>,
    ) -> Result<(), StoreError> {
        let action_id = action_id.to_owned();
        self.access(move |connection| {
            let changed = connection.execute(
                "UPDATE surface_actions
                 SET state = ?1, telegram_message_id = ?2, error = ?3, updated_at_ms = ?4
                 WHERE action_id = ?5 AND state = ?6",
                params![to, telegram_message_id, error, now_ms()?, action_id, from],
            )?;
            if changed == 1 {
                Ok(())
            } else {
                Err(StoreError::Invalid(format!(
                    "surface action cannot move from {from} to {to}"
                )))
            }
        })
        .await
    }
}

fn validate_action(action: &ActionLink) -> Result<(), StoreError> {
    let bounded = !action.id.is_empty()
        && action.id.len() <= 1024
        && !action.title.is_empty()
        && action.title.len() <= 256
        && !action.message.is_empty()
        && action.message.len() <= 2048
        && !action.button.is_empty()
        && action.button.len() <= 64
        && action.url.as_str().len() <= 16 * 1024;
    let safe_text = [&action.id, &action.title, &action.message, &action.button]
        .into_iter()
        .all(|value| {
            !value
                .bytes()
                .any(|byte| byte.is_ascii_control() && byte != b'\n')
        });
    if !bounded
        || !safe_text
        || action.url.scheme() != "https"
        || action.url.host_str().is_none()
        || !action.url.username().is_empty()
        || action.url.password().is_some()
        || action
            .url
            .fragment()
            .is_some_and(|fragment| !action.sensitive_fragment || !valid_setup_fragment(fragment))
        || action.expires_at_ms.is_some_and(|expiry| expiry <= 0)
    {
        return Err(StoreError::Invalid(
            "surface action is malformed or unsafe".to_owned(),
        ));
    }
    Ok(())
}

fn valid_setup_fragment(fragment: &str) -> bool {
    let mut version = None;
    let mut key = None;
    let mut token = None;
    for (name, value) in url::form_urlencoded::parse(fragment.as_bytes()) {
        let slot = match name.as_ref() {
            "v" => &mut version,
            "key" => &mut key,
            "token" => &mut token,
            _ => return false,
        };
        if slot.replace(value.into_owned()).is_some() {
            return false;
        }
    }
    version.as_deref() == Some("1")
        && key.as_deref().is_some_and(valid_secret_hex)
        && token.as_deref().is_some_and(valid_secret_hex)
}

fn valid_secret_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
