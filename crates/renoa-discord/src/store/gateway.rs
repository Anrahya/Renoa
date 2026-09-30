//! The connected bot's identity and the Discord gateway's resume cursor.

use rusqlite::{OptionalExtension as _, params};

use super::SurfaceStore;
use crate::{DiscordError, snowflake::Snowflake};

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
}
