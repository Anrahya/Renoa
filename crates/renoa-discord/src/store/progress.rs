//! Where a running command's transient progress is shown, and which Discord
//! message shows it. The message is recorded once it is posted, so a restarted
//! surface still deletes it once its command finishes.

use rusqlite::{OptionalExtension as _, params};

use super::SurfaceStore;
use crate::DiscordError;

/// Where a command's transient progress is shown: its task's channel and, for
/// a command from this surface, the Discord message it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgressTarget {
    pub(crate) channel_id: String,
    pub(crate) reply_to: Option<String>,
}

/// A posted progress message that has not been deleted yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShownProgress {
    pub(crate) command_id: String,
    pub(crate) channel_id: String,
    pub(crate) message_id: String,
    /// Whether the command has finished, or is no longer known.
    pub(crate) finished: bool,
}

impl SurfaceStore {
    /// Where an applied command's progress belongs, if the command is known.
    pub(crate) fn progress_target(
        &self,
        command_id: &str,
    ) -> Result<Option<ProgressTarget>, DiscordError> {
        let command_id = command_id.to_owned();
        self.access(move |connection| {
            connection
                .query_row(
                    "SELECT tasks.channel_id, turns.message_id
                     FROM replies JOIN tasks ON tasks.task_id = replies.task_id
                     LEFT JOIN turns ON turns.command_id = replies.command_id
                     WHERE replies.command_id = ?1",
                    [&command_id],
                    |row| {
                        Ok(ProgressTarget {
                            channel_id: row.get(0)?,
                            reply_to: row.get(1)?,
                        })
                    },
                )
                .optional()
                .map_err(DiscordError::from)
        })
    }

    pub(crate) fn record_progress_message(
        &self,
        command_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(), DiscordError> {
        let row = [command_id, channel_id, message_id].map(str::to_owned);
        self.access(move |connection| {
            connection.execute(
                "INSERT OR REPLACE INTO progress_messages(command_id, channel_id, message_id)
                 VALUES (?1, ?2, ?3)",
                params![row[0], row[1], row[2]],
            )?;
            Ok(())
        })
    }

    pub(crate) fn clear_progress_message(&self, command_id: &str) -> Result<(), DiscordError> {
        let command_id = command_id.to_owned();
        self.access(move |connection| {
            connection.execute(
                "DELETE FROM progress_messages WHERE command_id = ?1",
                [command_id],
            )?;
            Ok(())
        })
    }

    pub(crate) fn shown_progress(&self) -> Result<Vec<ShownProgress>, DiscordError> {
        self.access(|connection| {
            let mut statement = connection.prepare(
                "SELECT progress_messages.command_id, progress_messages.channel_id,
                        progress_messages.message_id, COALESCE(replies.finished, 1)
                 FROM progress_messages
                 LEFT JOIN replies ON replies.command_id = progress_messages.command_id
                 ORDER BY progress_messages.command_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(ShownProgress {
                    command_id: row.get(0)?,
                    channel_id: row.get(1)?,
                    message_id: row.get(2)?,
                    finished: row.get(3)?,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }
}
