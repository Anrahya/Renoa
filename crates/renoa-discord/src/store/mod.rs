use std::{fs::File, path::PathBuf};

use rusqlite::OptionalExtension as _;

use uuid::Uuid;

use crate::{DiscordError, snowflake::Snowflake};

mod schema;
mod turns;

#[cfg(test)]
mod tests;

pub(crate) use turns::{Enqueue, GatewayCursor, Outbound, QueuedTurn};

#[derive(Debug)]
pub(crate) struct SurfaceStore {
    database: PathBuf,
    _lease: File,
}

pub(crate) struct IncomingMessage {
    pub(crate) message_id: Snowflake,
    pub(crate) channel_id: Snowflake,
    pub(crate) author_id: Snowflake,
    pub(crate) canonical: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    Accepted,
    Duplicate,
}

impl SurfaceStore {
    pub(crate) fn open(data_directory: &std::path::Path) -> Result<Self, DiscordError> {
        let surface_directory = data_directory.join("surfaces").join("discord");
        std::fs::create_dir_all(&surface_directory)?;
        schema::restrict_directory(&surface_directory)?;
        let lease = schema::acquire_lease(&surface_directory)?;
        let database = surface_directory.join(schema::DATABASE_FILE);
        drop(schema::open(&database)?);
        schema::restrict_database(&database)?;
        Ok(Self {
            database,
            _lease: lease,
        })
    }

    pub(crate) fn bind_identity(
        &self,
        guild_id: &Snowflake,
        operator_user_id: &Snowflake,
        agent_id: Uuid,
    ) -> Result<(), DiscordError> {
        let guild_id = guild_id.as_str().to_owned();
        let operator_user_id = operator_user_id.as_str().to_owned();
        let agent_id = agent_id.to_string();
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let existing = transaction
                .query_row(
                    "SELECT guild_id, operator_user_id, agent_id FROM identity WHERE singleton = 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            match existing {
                Some((stored_guild, stored_operator, stored_agent))
                    if stored_guild == guild_id
                        && stored_operator == operator_user_id
                        && stored_agent == agent_id => {}
                Some(_) => {
                    return Err(DiscordError::Invalid(
                        "stored Discord guild, operator, or agent differs from this launch configuration"
                            .to_owned(),
                    ));
                }
                None => {
                    transaction.execute(
                        "INSERT INTO identity(singleton, guild_id, operator_user_id, agent_id)
                         VALUES (1, ?1, ?2, ?3)",
                        rusqlite::params![guild_id, operator_user_id, agent_id],
                    )?;
                }
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub(crate) fn admit(&self, message: IncomingMessage) -> Result<Admission, DiscordError> {
        if message.canonical.is_empty() {
            return Err(DiscordError::Invalid(
                "Discord message canonical payload must not be empty".to_owned(),
            ));
        }
        let message_id = message.message_id.as_str().to_owned();
        let channel_id = message.channel_id.as_str().to_owned();
        let author_id = message.author_id.as_str().to_owned();
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            if transaction
                .query_row("SELECT 1 FROM identity WHERE singleton = 1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()?
                .is_none()
            {
                return Err(DiscordError::Invalid(
                    "Discord surface identity is not bound".to_owned(),
                ));
            }
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
                    || stored_canonical != message.canonical
                {
                    return Err(DiscordError::Invalid(format!(
                        "Discord reused message {message_id} with different content"
                    )));
                }
                transaction.commit()?;
                return Ok(Admission::Duplicate);
            }
            transaction.execute(
                "INSERT INTO messages(
                    message_id, channel_id, author_id, canonical, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    message_id,
                    channel_id,
                    author_id,
                    message.canonical,
                    schema::now_ms()?,
                ],
            )?;
            transaction.commit()?;
            Ok(Admission::Accepted)
        })
    }
}

impl SurfaceStore {
    fn access<T>(
        &self,
        action: impl FnOnce(&mut rusqlite::Connection) -> Result<T, DiscordError>,
    ) -> Result<T, DiscordError> {
        let mut connection = schema::open(&self.database)?;
        action(&mut connection)
    }
}
