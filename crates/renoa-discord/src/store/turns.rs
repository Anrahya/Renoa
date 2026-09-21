use rusqlite::{OptionalExtension as _, params};
use uuid::Uuid;

use super::SurfaceStore;
use crate::{DiscordError, snowflake::Snowflake, store::schema};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Enqueue {
    Fresh,
    Duplicate,
}

#[derive(Debug)]
pub(crate) struct QueuedTurn {
    pub(crate) message_id: String,
    pub(crate) session_id: Uuid,
    pub(crate) request_id: Uuid,
    pub(crate) prompt: String,
    pub(crate) observed_at_ms: i64,
}

#[derive(Debug)]
pub(crate) struct Outbound {
    pub(crate) message_id: String,
    pub(crate) channel_id: String,
    pub(crate) chunk: i64,
    pub(crate) body: String,
}

#[derive(Debug)]
pub(crate) struct GatewayCursor {
    pub(crate) session_id: Option<String>,
    pub(crate) resume_url: Option<String>,
    pub(crate) sequence: Option<i64>,
}

impl SurfaceStore {
    pub(crate) fn remember_bot(&self, bot_user_id: &Snowflake) -> Result<(), DiscordError> {
        let bot_user_id = bot_user_id.as_str().to_owned();
        self.access(move |connection| {
            let changed = connection.execute(
                "UPDATE identity SET bot_user_id = ?1
                 WHERE singleton = 1 AND (bot_user_id IS NULL OR bot_user_id = ?1)",
                [&bot_user_id],
            )?;
            if changed == 1 {
                Ok(())
            } else {
                Err(DiscordError::Invalid(
                    "stored Discord bot user differs from the connected bot".to_owned(),
                ))
            }
        })
    }

    pub(crate) fn bot_user_id(&self) -> Result<Option<String>, DiscordError> {
        self.access(|connection| {
            connection
                .query_row(
                    "SELECT bot_user_id FROM identity WHERE singleton = 1",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .map(std::option::Option::flatten)
                .map_err(DiscordError::from)
        })
    }

    pub(crate) fn enqueue(
        &self,
        message_id: &Snowflake,
        channel_id: &Snowflake,
        author_id: &Snowflake,
        canonical: &[u8],
        prompt: &str,
    ) -> Result<Enqueue, DiscordError> {
        if canonical.is_empty() {
            return Err(DiscordError::Invalid(
                "Discord message canonical payload must not be empty".to_owned(),
            ));
        }
        let message_id = message_id.as_str().to_owned();
        let channel_id = channel_id.as_str().to_owned();
        let author_id = author_id.as_str().to_owned();
        let canonical = canonical.to_vec();
        let prompt = prompt.to_owned();
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let existing = transaction
                .query_row(
                    "SELECT canonical FROM messages WHERE message_id = ?1",
                    [&message_id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()?;
            if let Some(stored) = existing {
                if stored != canonical {
                    return Err(DiscordError::Invalid(format!(
                        "Discord reused message {message_id} with different content"
                    )));
                }
                transaction.commit()?;
                return Ok(Enqueue::Duplicate);
            }
            let session_id = conversation_session(&transaction, &channel_id)?;
            let request_id = Uuid::new_v4().to_string();
            let now = schema::now_ms()?;
            transaction.execute(
                "INSERT INTO messages(
                    message_id, channel_id, author_id, canonical, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![message_id, channel_id, author_id, canonical, now],
            )?;
            transaction.execute(
                "INSERT INTO turns(message_id, session_id, request_id, prompt, state)
                 VALUES (?1, ?2, ?3, ?4, 'queued')",
                params![message_id, session_id, request_id, prompt],
            )?;
            transaction.commit()?;
            Ok(Enqueue::Fresh)
        })
    }

    pub(crate) fn recover(&self) -> Result<(), DiscordError> {
        self.access(|connection| {
            let transaction = schema::immediate_transaction(connection)?;
            transaction.execute(
                "UPDATE turns SET state = 'queued' WHERE state = 'running'",
                [],
            )?;
            transaction.execute(
                "UPDATE deliveries SET state = 'unknown' WHERE state = 'sending'",
                [],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub(crate) fn next_queued(&self) -> Result<Option<QueuedTurn>, DiscordError> {
        self.access(|connection| {
            connection
                .query_row(
                    "SELECT turns.message_id, turns.session_id, turns.request_id, turns.prompt,
                            messages.created_at_ms
                     FROM turns JOIN messages ON messages.message_id = turns.message_id
                     WHERE turns.state = 'queued' ORDER BY turns.message_id LIMIT 1",
                    [],
                    |row| {
                        Ok(QueuedTurn {
                            message_id: row.get(0)?,
                            session_id: parse_uuid(&row.get::<_, String>(1)?)?,
                            request_id: parse_uuid(&row.get::<_, String>(2)?)?,
                            prompt: row.get(3)?,
                            observed_at_ms: row.get(4)?,
                        })
                    },
                )
                .optional()
                .map_err(DiscordError::from)
        })
    }

    pub(crate) fn mark_running(&self, message_id: &str) -> Result<(), DiscordError> {
        self.transition_turn(message_id, "queued", "running", None)
    }

    pub(crate) fn mark_ready(
        &self,
        message_id: &str,
        result: &str,
        pages: &[String],
    ) -> Result<(), DiscordError> {
        let message_id = message_id.to_owned();
        let result = result.to_owned();
        let pages = pages.to_vec();
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let changed = transaction.execute(
                "UPDATE turns SET state = 'ready', result = ?1
                 WHERE message_id = ?2 AND state = 'running'",
                params![result, message_id],
            )?;
            require_one(changed, &message_id)?;
            for (chunk, body) in pages.iter().enumerate() {
                let chunk = i64::try_from(chunk).map_err(|_| {
                    DiscordError::Invalid("Discord reply has too many pages".to_owned())
                })?;
                transaction.execute(
                    "INSERT INTO deliveries(message_id, chunk, body, state) VALUES (?1, ?2, ?3, 'pending')",
                    params![message_id, chunk, body],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub(crate) fn next_outbound(&self) -> Result<Option<Outbound>, DiscordError> {
        self.access(|connection| {
            connection
                .query_row(
                    "SELECT deliveries.message_id, messages.channel_id, deliveries.chunk, deliveries.body
                     FROM deliveries
                     JOIN messages ON messages.message_id = deliveries.message_id
                     WHERE deliveries.state = 'pending'
                       AND NOT EXISTS (
                         SELECT 1 FROM deliveries earlier
                         WHERE earlier.message_id = deliveries.message_id
                           AND earlier.chunk < deliveries.chunk
                           AND earlier.state <> 'sent'
                       )
                     ORDER BY deliveries.message_id, deliveries.chunk LIMIT 1",
                    [],
                    |row| {
                        Ok(Outbound {
                            message_id: row.get(0)?,
                            channel_id: row.get(1)?,
                            chunk: row.get(2)?,
                            body: row.get(3)?,
                        })
                    },
                )
                .optional()
                .map_err(DiscordError::from)
        })
    }

    pub(crate) fn mark_sending(&self, message_id: &str, chunk: i64) -> Result<(), DiscordError> {
        self.transition_delivery(message_id, chunk, "pending", "sending", None)
    }

    pub(crate) fn mark_sent(
        &self,
        message_id: &str,
        chunk: i64,
        reply_id: &Snowflake,
    ) -> Result<(), DiscordError> {
        self.transition_delivery(
            message_id,
            chunk,
            "sending",
            "sent",
            Some(reply_id.as_str()),
        )
    }

    pub(crate) fn mark_unknown(&self, message_id: &str, chunk: i64) -> Result<(), DiscordError> {
        self.transition_delivery(message_id, chunk, "sending", "unknown", None)
    }

    pub(crate) fn release_sending(&self, message_id: &str, chunk: i64) -> Result<(), DiscordError> {
        self.transition_delivery(message_id, chunk, "sending", "pending", None)
    }

    pub(crate) fn mark_failed(&self, message_id: &str, chunk: i64) -> Result<(), DiscordError> {
        self.transition_delivery(message_id, chunk, "sending", "failed", None)
    }

    pub(crate) fn load_gateway(&self) -> Result<GatewayCursor, DiscordError> {
        self.access(|connection| {
            let row = connection
                .query_row(
                    "SELECT session_id, resume_url, sequence FROM gateway WHERE singleton = 1",
                    [],
                    |row| {
                        Ok(GatewayCursor {
                            session_id: row.get(0)?,
                            resume_url: row.get(1)?,
                            sequence: row.get(2)?,
                        })
                    },
                )
                .optional()?;
            Ok(row.unwrap_or(GatewayCursor {
                session_id: None,
                resume_url: None,
                sequence: None,
            }))
        })
    }

    pub(crate) fn save_gateway(&self, cursor: GatewayCursor) -> Result<(), DiscordError> {
        self.access(move |connection| {
            connection.execute(
                "INSERT INTO gateway(singleton, session_id, resume_url, sequence)
                 VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT(singleton) DO UPDATE SET
                    session_id = excluded.session_id,
                    resume_url = excluded.resume_url,
                    sequence = excluded.sequence",
                params![cursor.session_id, cursor.resume_url, cursor.sequence],
            )?;
            Ok(())
        })
    }

    fn transition_turn(
        &self,
        message_id: &str,
        from: &str,
        to: &str,
        result: Option<String>,
    ) -> Result<(), DiscordError> {
        let message_id = message_id.to_owned();
        let from = from.to_owned();
        let to = to.to_owned();
        self.access(move |connection| {
            let changed = connection.execute(
                "UPDATE turns SET state = ?1, result = ?2 WHERE message_id = ?3 AND state = ?4",
                params![to, result, message_id, from],
            )?;
            require_one(changed, &message_id)
        })
    }

    fn transition_delivery(
        &self,
        message_id: &str,
        chunk: i64,
        from: &str,
        to: &str,
        reply_id: Option<&str>,
    ) -> Result<(), DiscordError> {
        let message_id = message_id.to_owned();
        let from = from.to_owned();
        let to = to.to_owned();
        let reply_id = reply_id.map(str::to_owned);
        self.access(move |connection| {
            let changed = connection.execute(
                "UPDATE deliveries SET state = ?1, reply_id = ?2
                 WHERE message_id = ?3 AND chunk = ?4 AND state = ?5",
                params![to, reply_id, message_id, chunk, from],
            )?;
            require_one(changed, &message_id)
        })
    }
}

fn conversation_session(
    connection: &rusqlite::Connection,
    channel_id: &str,
) -> Result<String, DiscordError> {
    if let Some(session_id) = connection
        .query_row(
            "SELECT session_id FROM conversations WHERE channel_id = ?1",
            [channel_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(session_id);
    }
    let session_id = Uuid::new_v4().to_string();
    connection.execute(
        "INSERT INTO conversations(channel_id, session_id) VALUES (?1, ?2)",
        params![channel_id, session_id],
    )?;
    Ok(session_id)
}

fn parse_uuid(value: &str) -> Result<Uuid, rusqlite::Error> {
    Uuid::parse_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn require_one(changed: usize, message_id: &str) -> Result<(), DiscordError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(DiscordError::Invalid(format!(
            "Discord message {message_id} was not in the expected state"
        )))
    }
}
