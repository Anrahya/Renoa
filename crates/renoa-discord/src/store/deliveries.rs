//! Reply pages post in order; an interrupted or rejected page is never blindly
//! repeated and fails the pages behind it.

use rusqlite::{OptionalExtension as _, params};

use super::{SurfaceStore, schema, turns::require_one};
use crate::{DiscordError, snowflake::Snowflake};

#[derive(Debug)]
pub(crate) struct Outbound {
    pub(crate) command_id: String,
    pub(crate) chunk: i64,
    pub(crate) channel_id: String,
    pub(crate) reply_to: Option<String>,
    pub(crate) body: String,
}

impl SurfaceStore {
    /// Marks deliveries interrupted mid-send as unknown so they are never
    /// blindly repeated, and fails the pages queued behind them.
    pub(crate) fn recover(&self) -> Result<(), DiscordError> {
        self.access(|connection| {
            let transaction = schema::immediate_transaction(connection)?;
            transaction.execute(
                "UPDATE deliveries SET state = 'unknown' WHERE state = 'sending'",
                [],
            )?;
            strand_blocked_pages(&transaction)?;
            transaction.commit()?;
            Ok(())
        })
    }
    pub(crate) fn next_outbound(&self) -> Result<Option<Outbound>, DiscordError> {
        self.access(|connection| {
            connection
                .query_row(
                    "SELECT command_id, chunk, channel_id, reply_to, body FROM deliveries
                     WHERE state = 'pending'
                       AND NOT EXISTS (
                         SELECT 1 FROM deliveries earlier
                         WHERE earlier.command_id = deliveries.command_id
                           AND earlier.chunk < deliveries.chunk
                           AND earlier.state <> 'sent'
                       )
                     ORDER BY rowid LIMIT 1",
                    [],
                    |row| {
                        Ok(Outbound {
                            command_id: row.get(0)?,
                            chunk: row.get(1)?,
                            channel_id: row.get(2)?,
                            reply_to: row.get(3)?,
                            body: row.get(4)?,
                        })
                    },
                )
                .optional()
                .map_err(DiscordError::from)
        })
    }
    pub(crate) fn mark_sending(&self, command_id: &str, chunk: i64) -> Result<(), DiscordError> {
        self.transition_delivery(command_id, chunk, "pending", "sending", None)
    }
    pub(crate) fn mark_sent(
        &self,
        command_id: &str,
        chunk: i64,
        reply_id: &Snowflake,
    ) -> Result<(), DiscordError> {
        self.transition_delivery(
            command_id,
            chunk,
            "sending",
            "sent",
            Some(reply_id.as_str()),
        )
    }
    pub(crate) fn mark_unknown(&self, command_id: &str, chunk: i64) -> Result<(), DiscordError> {
        self.finish_delivery(command_id, chunk, "unknown")
    }
    pub(crate) fn release_sending(&self, command_id: &str, chunk: i64) -> Result<(), DiscordError> {
        self.transition_delivery(command_id, chunk, "sending", "pending", None)
    }
    pub(crate) fn mark_failed(&self, command_id: &str, chunk: i64) -> Result<(), DiscordError> {
        self.finish_delivery(command_id, chunk, "failed")
    }
    fn finish_delivery(
        &self,
        command_id: &str,
        chunk: i64,
        state: &str,
    ) -> Result<(), DiscordError> {
        let command_id = command_id.to_owned();
        let state = state.to_owned();
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let changed = transaction.execute(
                "UPDATE deliveries SET state = ?1
                 WHERE command_id = ?2 AND chunk = ?3 AND state = 'sending'",
                params![state, command_id, chunk],
            )?;
            require_one(changed, &command_id)?;
            transaction.execute(
                "UPDATE deliveries SET state = 'failed'
                 WHERE command_id = ?1 AND chunk > ?2 AND state = 'pending'",
                params![command_id, chunk],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }
    pub(crate) fn has_reply(&self, message_id: &str) -> Result<bool, DiscordError> {
        let message_id = message_id.to_owned();
        self.access(move |connection| {
            connection
                .query_row(
                    "SELECT 1 FROM deliveries WHERE reply_id = ?1",
                    [message_id],
                    |_| Ok(()),
                )
                .optional()
                .map(|row| row.is_some())
                .map_err(DiscordError::from)
        })
    }
    fn transition_delivery(
        &self,
        command_id: &str,
        chunk: i64,
        from: &str,
        to: &str,
        reply_id: Option<&str>,
    ) -> Result<(), DiscordError> {
        let command_id = command_id.to_owned();
        let from = from.to_owned();
        let to = to.to_owned();
        let reply_id = reply_id.map(str::to_owned);
        self.access(move |connection| {
            let changed = connection.execute(
                "UPDATE deliveries SET state = ?1, reply_id = ?2
                 WHERE command_id = ?3 AND chunk = ?4 AND state = ?5",
                params![to, reply_id, command_id, chunk, from],
            )?;
            require_one(changed, &command_id)
        })
    }
}

fn strand_blocked_pages(connection: &rusqlite::Connection) -> Result<(), DiscordError> {
    connection.execute(
        "UPDATE deliveries SET state = 'failed'
         WHERE state = 'pending'
           AND EXISTS (
             SELECT 1 FROM deliveries earlier
             WHERE earlier.command_id = deliveries.command_id
               AND earlier.chunk < deliveries.chunk
               AND earlier.state IN ('unknown', 'failed')
           )",
        [],
    )?;
    Ok(())
}
