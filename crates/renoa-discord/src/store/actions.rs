use super::{SurfaceStore, schema};
use crate::DiscordError;
use rusqlite::{OptionalExtension as _, params};

impl SurfaceStore {
    pub(crate) fn claim_action(
        &self,
        message: &str,
        call: &str,
        stage: &str,
        digest: &[u8],
    ) -> Result<bool, DiscordError> {
        self.access(|connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let existing: Option<(Vec<u8>, String)> = transaction.query_row(
                "SELECT digest, state FROM actions WHERE message_id = ?1 AND call_id = ?2 AND stage = ?3",
                params![message, call, stage], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            if let Some((saved, state)) = existing {
                if saved != digest || state != "sent" {
                    return Err(DiscordError::Invalid("Setup message delivery is unconfirmed. Check your DM; restart setup if needed. Renoa will not blindly repeat it.".into()));
                }
                return Ok(false);
            }
            transaction.execute("INSERT INTO actions(message_id, call_id, stage, digest, state) VALUES (?1, ?2, ?3, ?4, 'sending')", params![message, call, stage, digest])?;
            transaction.commit()?;
            Ok(true)
        })
    }

    pub(crate) fn action_state(
        &self,
        message: &str,
        call: &str,
        stage: &str,
        outcome: &str,
    ) -> Result<(), DiscordError> {
        self.access(|connection| {
            let changed = connection.execute("UPDATE actions SET state = ?4 WHERE message_id = ?1 AND call_id = ?2 AND stage = ?3 AND state = 'sending'", params![message, call, stage, outcome])?;
            if changed != 1 { return Err(DiscordError::Invalid("Setup delivery was not in the expected state".into())); }
            Ok(())
        })
    }

    pub(crate) fn release_action(
        &self,
        message: &str,
        call: &str,
        stage: &str,
    ) -> Result<(), DiscordError> {
        self.access(|connection| {
            connection.execute("DELETE FROM actions WHERE message_id = ?1 AND call_id = ?2 AND stage = ?3 AND state = 'sending'", params![message, call, stage])?;
            Ok(())
        })
    }
}
