use rusqlite::{OptionalExtension as _, params};
use uuid::Uuid;

use super::SurfaceStore;
use crate::{DiscordError, ingress::pages, snowflake::Snowflake, store::schema};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Enqueue {
    Fresh,
    Duplicate,
}

/// One Discord message waiting to become an RCP command.
#[derive(Debug)]
pub(crate) struct QueuedTurn {
    pub(crate) message_id: String,
    pub(crate) task_id: Uuid,
    pub(crate) agent_id: Uuid,
    pub(crate) command_id: Uuid,
    pub(crate) prompt: String,
    pub(crate) opened: bool,
}

impl SurfaceStore {
    /// Records one addressed Discord message as a queued command on its
    /// channel's current task. A channel whose agent changed starts a new task.
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
                    "SELECT channel_id, author_id, canonical FROM messages WHERE message_id = ?1",
                    [&message_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((stored_channel, stored_author, stored_canonical)) = existing {
                if stored_channel != channel_id
                    || stored_author != author_id
                    || stored_canonical != canonical
                {
                    return Err(DiscordError::Invalid(format!(
                        "Discord reused message {message_id} with different content"
                    )));
                }
                transaction.commit()?;
                return Ok(Enqueue::Duplicate);
            }
            let agent_id: String = transaction.query_row(
                "SELECT COALESCE(
                    (SELECT agent_id FROM channel_bindings WHERE channel_id = ?1), agent_id
                 ) FROM identity WHERE singleton = 1",
                [&channel_id],
                |row| row.get(0),
            )?;
            let task_id = current_task(&transaction, &channel_id, &agent_id)?;
            transaction.execute(
                "INSERT INTO messages(
                    message_id, channel_id, author_id, canonical, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    message_id,
                    channel_id,
                    author_id,
                    canonical,
                    schema::now_ms()?
                ],
            )?;
            transaction.execute(
                "INSERT INTO turns(message_id, task_id, command_id, prompt, state)
                 VALUES (?1, ?2, ?3, ?4, 'queued')",
                params![message_id, task_id, Uuid::new_v4().to_string(), prompt],
            )?;
            transaction.commit()?;
            Ok(Enqueue::Fresh)
        })
    }

    pub(crate) fn next_queued(&self) -> Result<Option<QueuedTurn>, DiscordError> {
        self.access(|connection| {
            connection
                .query_row(
                    "SELECT turns.message_id, turns.task_id, tasks.agent_id, turns.command_id,
                            turns.prompt, tasks.opened
                     FROM turns JOIN tasks ON tasks.task_id = turns.task_id
                     WHERE turns.state = 'queued'
                     ORDER BY length(turns.message_id), turns.message_id LIMIT 1",
                    [],
                    |row| {
                        Ok(QueuedTurn {
                            message_id: row.get(0)?,
                            task_id: parse_uuid(&row.get::<_, String>(1)?)?,
                            agent_id: parse_uuid(&row.get::<_, String>(2)?)?,
                            command_id: parse_uuid(&row.get::<_, String>(3)?)?,
                            prompt: row.get(4)?,
                            opened: row.get(5)?,
                        })
                    },
                )
                .optional()
                .map_err(DiscordError::from)
        })
    }

    pub(crate) fn mark_opened(&self, task_id: Uuid) -> Result<(), DiscordError> {
        self.access(move |connection| {
            connection.execute(
                "UPDATE tasks SET opened = 1 WHERE task_id = ?1",
                [task_id.to_string()],
            )?;
            Ok(())
        })
    }

    /// Every opened task and the last task sequence this surface applied.
    pub(crate) fn opened_tasks(&self) -> Result<Vec<(Uuid, Option<u64>)>, DiscordError> {
        self.access(|connection| {
            let mut statement = connection
                .prepare("SELECT task_id, cursor FROM tasks WHERE opened = 1 ORDER BY task_id")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    parse_uuid(&row.get::<_, String>(0)?)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            })?;
            let mut tasks = Vec::new();
            for row in rows {
                let (task_id, cursor) = row?;
                tasks.push((task_id, cursor.map(to_sequence).transpose()?));
            }
            Ok(tasks)
        })
    }

    pub(crate) fn mark_submitted(&self, message_id: &str) -> Result<(), DiscordError> {
        let message_id = message_id.to_owned();
        self.access(move |connection| {
            let changed = connection.execute(
                "UPDATE turns SET state = 'submitted' WHERE message_id = ?1 AND state = 'queued'",
                [&message_id],
            )?;
            require_one(changed, &message_id)
        })
    }

    /// Answers a queued message without submitting it, for example when its
    /// agent is offline.
    pub(crate) fn answer_locally(&self, message_id: &str, text: &str) -> Result<(), DiscordError> {
        let message_id = message_id.to_owned();
        let text = text.to_owned();
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let (command_id, channel_id): (String, String) = transaction.query_row(
                "SELECT turns.command_id, tasks.channel_id
                 FROM turns JOIN tasks ON tasks.task_id = turns.task_id
                 WHERE turns.message_id = ?1 AND turns.state = 'queued'",
                [&message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            transaction.execute(
                "UPDATE turns SET state = 'answered' WHERE message_id = ?1",
                [&message_id],
            )?;
            insert_pages(
                &transaction,
                &command_id,
                &channel_id,
                Some(&message_id),
                &text,
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// Whether the channel already has a conversation, so a thread keeps
    /// answering without a new mention.
    pub(crate) fn has_conversation(&self, channel_id: &str) -> Result<bool, DiscordError> {
        let channel_id = channel_id.to_owned();
        self.access(move |connection| {
            connection
                .query_row(
                    "SELECT 1 FROM tasks WHERE channel_id = ?1 AND current = 1",
                    [channel_id],
                    |_| Ok(()),
                )
                .optional()
                .map(|row| row.is_some())
                .map_err(DiscordError::from)
        })
    }
}

/// The channel's current task on `agent_id`, starting a new one when the
/// channel has none or its agent changed.
fn current_task(
    connection: &rusqlite::Connection,
    channel_id: &str,
    agent_id: &str,
) -> Result<String, DiscordError> {
    let current = connection
        .query_row(
            "SELECT task_id, agent_id FROM tasks WHERE channel_id = ?1 AND current = 1",
            [channel_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((task_id, current_agent)) = &current
        && current_agent == agent_id
    {
        return Ok(task_id.clone());
    }
    connection.execute(
        "UPDATE tasks SET current = 0 WHERE channel_id = ?1 AND current = 1",
        [channel_id],
    )?;
    let task_id = Uuid::new_v4().to_string();
    connection.execute(
        "INSERT INTO tasks(task_id, channel_id, agent_id, current) VALUES (?1, ?2, ?3, 1)",
        params![task_id, channel_id, agent_id],
    )?;
    Ok(task_id)
}

pub(super) fn insert_pages(
    connection: &rusqlite::Connection,
    command_id: &str,
    channel_id: &str,
    reply_to: Option<&str>,
    text: &str,
) -> Result<(), DiscordError> {
    for (chunk, body) in pages(text).iter().enumerate() {
        let chunk = i64::try_from(chunk)
            .map_err(|_| DiscordError::Invalid("Discord reply has too many pages".to_owned()))?;
        connection.execute(
            "INSERT INTO deliveries(command_id, chunk, channel_id, reply_to, body, state)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
            params![
                command_id,
                chunk,
                channel_id,
                (chunk == 0).then_some(reply_to).flatten(),
                body
            ],
        )?;
    }
    Ok(())
}

fn parse_uuid(value: &str) -> Result<Uuid, rusqlite::Error> {
    Uuid::parse_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn to_sequence(value: i64) -> Result<u64, rusqlite::Error> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

pub(super) fn require_one(changed: usize, key: &str) -> Result<(), DiscordError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(DiscordError::Invalid(format!(
            "Discord record {key} was not in the expected state"
        )))
    }
}
