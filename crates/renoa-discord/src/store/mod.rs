use std::{fs::File, path::PathBuf};

use rusqlite::OptionalExtension as _;

use uuid::Uuid;

use crate::{DiscordError, snowflake::Snowflake};

mod actions;
mod bindings;
mod deliveries;
mod gateway;
mod places;
mod progress;
mod replies;
mod schema;
mod turns;

#[cfg(test)]
mod routing_tests;
#[cfg(test)]
mod tests;

pub(crate) use deliveries::Outbound;
pub(crate) use gateway::GatewayCursor;
pub(crate) use progress::{ProgressTarget, ShownProgress};
pub(crate) use replies::Applied;
pub(crate) use turns::{Enqueue, QueuedTurn};

#[derive(Debug)]
pub(crate) struct SurfaceStore {
    database: PathBuf,
    _lease: Option<File>,
}

impl SurfaceStore {
    pub(crate) fn open(data_directory: &std::path::Path) -> Result<Self, DiscordError> {
        let surface_directory = data_directory.join("state/surfaces").join("discord");
        std::fs::create_dir_all(&surface_directory)?;
        schema::restrict_directory(&surface_directory)?;
        let lease = schema::acquire_lease(&surface_directory)?;
        let database = surface_directory.join(schema::DATABASE_FILE);
        drop(schema::open(&database)?);
        schema::restrict_database(&database)?;
        Ok(Self {
            database,
            _lease: Some(lease),
        })
    }

    pub(crate) fn control(data_directory: &std::path::Path) -> Result<Self, DiscordError> {
        let surface_directory = data_directory.join("state/surfaces/discord");
        std::fs::create_dir_all(&surface_directory)?;
        schema::restrict_directory(&surface_directory)?;
        let database = surface_directory.join(schema::DATABASE_FILE);
        drop(schema::open(&database)?);
        schema::restrict_database(&database)?;
        Ok(Self {
            database,
            _lease: None,
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
                        "stored Discord guild, operator, or agent differs from the Discord connection"
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
