use rusqlite::{OptionalExtension as _, params};

use super::{SurfaceStore, schema};
use crate::{
    DiscordError,
    places::{Changes, Place},
};

impl SurfaceStore {
    /// Applies one gateway dispatch's directory changes together.
    pub(crate) fn apply_places(&self, changes: &Changes) -> Result<(), DiscordError> {
        self.access(|connection| {
            let transaction = schema::immediate_transaction(connection)?;
            for place in &changes.known {
                remember(&transaction, place)?;
            }
            for channel_id in &changes.gone {
                transaction.execute("DELETE FROM channels WHERE channel_id = ?1", [channel_id])?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub(crate) fn remember_place(&self, place: &Place) -> Result<(), DiscordError> {
        self.access(|connection| remember(connection, place))
    }

    pub(crate) fn place(&self, channel_id: &str) -> Result<Option<Place>, DiscordError> {
        self.access(|connection| {
            connection
                .query_row(
                    "SELECT channel_id, name, thread_parent_id FROM channels WHERE channel_id = ?1",
                    [channel_id],
                    |row| {
                        Ok(Place {
                            channel_id: row.get(0)?,
                            name: row.get(1)?,
                            thread_parent_id: row.get(2)?,
                        })
                    },
                )
                .optional()
                .map_err(DiscordError::from)
        })
    }

    /// Whether messages in the channel go to a bound agent: its own binding,
    /// or its parent channel's for a thread.
    pub(crate) fn is_bound(&self, channel_id: &str) -> Result<bool, DiscordError> {
        self.access(|connection| Ok(bound_agent(connection, channel_id)?.is_some()))
    }
}

/// The agent bound to the channel that decides where a message goes: the
/// channel itself, or the parent channel of a thread.
pub(super) fn bound_agent(
    connection: &rusqlite::Connection,
    channel_id: &str,
) -> Result<Option<String>, DiscordError> {
    connection
        .query_row(
            "SELECT agent_id FROM channel_bindings WHERE channel_id = COALESCE(
                (SELECT thread_parent_id FROM channels WHERE channel_id = ?1), ?1
             )",
            [channel_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(DiscordError::from)
}

fn remember(connection: &rusqlite::Connection, place: &Place) -> Result<(), DiscordError> {
    connection.execute(
        "INSERT INTO channels(channel_id, name, thread_parent_id) VALUES (?1, ?2, ?3)
         ON CONFLICT(channel_id) DO UPDATE SET
            name = excluded.name, thread_parent_id = excluded.thread_parent_id",
        params![place.channel_id, place.name, place.thread_parent_id],
    )?;
    Ok(())
}
