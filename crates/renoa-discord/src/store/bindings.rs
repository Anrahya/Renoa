use super::{SurfaceStore, schema};
use crate::{
    DiscordError,
    control::{DiscordBinding, DiscordBindingRequest},
};
use rusqlite::{OptionalExtension as _, params};

impl SurfaceStore {
    pub(crate) fn channel_binding(
        &self,
        channel: &str,
    ) -> Result<Option<DiscordBinding>, DiscordError> {
        self.access(|connection| {
            connection.query_row("SELECT channel_id, agent_id, channel_name, revision FROM channel_bindings WHERE channel_id = ?1", [channel], read).optional().map_err(Into::into)
        })
    }

    pub(crate) fn bindings(&self) -> Result<Vec<DiscordBinding>, DiscordError> {
        self.access(|connection| {
            let mut statement = connection.prepare("SELECT channel_id, agent_id, channel_name, revision FROM channel_bindings ORDER BY channel_id")?;
            Ok(statement.query_map([], read)?.collect::<Result<_, _>>()?)
        })
    }

    pub(crate) fn binding_receipt(
        &self,
        request: &DiscordBindingRequest,
    ) -> Result<Option<DiscordBinding>, DiscordError> {
        self.access(|connection| receipt(connection, request))
    }

    pub(crate) fn bind_channel(
        &self,
        request: &DiscordBindingRequest,
        name: &str,
    ) -> Result<DiscordBinding, DiscordError> {
        self.access(|connection| {
            let transaction = schema::immediate_transaction(connection)?;
            if let Some(saved) = receipt(&transaction, request)? { return Ok(saved); }
            let revision: i64 = transaction.query_row("SELECT revision FROM channel_bindings WHERE channel_id = ?1", [&request.channel_id], |row| row.get(0)).optional()?.unwrap_or(0);
            if revision != request.expected_revision { return Err(DiscordError::Invalid("This channel binding changed. Refresh before saving.".into())); }
            let revision = revision.checked_add(1).ok_or_else(|| DiscordError::Invalid("Channel binding revision exhausted".into()))?;
            let result = DiscordBinding { channel_id: request.channel_id.clone(), agent_id: request.agent_id, channel_name: name.to_owned(), revision };
            transaction.execute("INSERT INTO channel_bindings(channel_id, agent_id, channel_name, revision) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(channel_id) DO UPDATE SET agent_id = excluded.agent_id, channel_name = excluded.channel_name, revision = excluded.revision", params![result.channel_id, result.agent_id.to_string(), result.channel_name, revision])?;
            transaction.execute("INSERT INTO binding_receipts(operation_id, request, result) VALUES (?1, ?2, ?3)", params![request.operation_id.to_string(), serde_json::to_string(request)?, serde_json::to_string(&result)?])?;
            transaction.commit()?;
            Ok(result)
        })
    }
}

fn receipt(
    connection: &rusqlite::Connection,
    request: &DiscordBindingRequest,
) -> Result<Option<DiscordBinding>, DiscordError> {
    let saved: Option<(String, String)> = connection
        .query_row(
            "SELECT request, result FROM binding_receipts WHERE operation_id = ?1",
            [request.operation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    saved
        .map(|(fields, result)| {
            if fields != serde_json::to_string(request)? {
                return Err(DiscordError::Invalid(
                    "Channel binding operation was reused with different fields".into(),
                ));
            }
            serde_json::from_str(&result).map_err(Into::into)
        })
        .transpose()
}

fn read(row: &rusqlite::Row<'_>) -> Result<DiscordBinding, rusqlite::Error> {
    let agent: String = row.get(1)?;
    let agent_id = uuid::Uuid::parse_str(&agent).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(DiscordBinding {
        channel_id: row.get(0)?,
        agent_id,
        channel_name: row.get(2)?,
        revision: row.get(3)?,
    })
}
