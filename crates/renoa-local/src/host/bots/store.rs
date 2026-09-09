use std::path::Path;

use rusqlite::{OptionalExtension as _, TransactionBehavior, params};

use super::{BotPage, BotRecord, BotSummary, LocalHostError, profile_id};
use crate::{AgentProfile, AgentProfileId, host::catalog};
use renoa_kernel::AgentId;
use tokio_util::sync::CancellationToken;

pub(super) fn ensure(
    path: &Path,
    record: BotRecord,
    cancellation: &CancellationToken,
) -> Result<BotRecord, LocalHostError> {
    require_active(cancellation)?;
    let mut connection = catalog::open_verified(path)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(catalog::HostCatalogError::from)?;
    publish(transaction, record, cancellation)
}

fn publish(
    transaction: rusqlite::Transaction<'_>,
    record: BotRecord,
    cancellation: &CancellationToken,
) -> Result<BotRecord, LocalHostError> {
    require_active(cancellation)?;
    let encoded = serde_json::to_string(&record)?;
    let existing: Option<String> = transaction
        .query_row(
            "SELECT record_json FROM host_bots WHERE agent_id=?1",
            [record.id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(catalog::HostCatalogError::from)?;
    if let Some(existing) = existing {
        let existing: BotRecord = serde_json::from_str(&existing)?;
        if existing != record {
            return Err(LocalHostError::AgentConflict(record.id));
        }
        transaction
            .commit()
            .map_err(catalog::HostCatalogError::from)?;
        return Ok(existing);
    }
    let profile = profile_id(record.id)?;
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM host_agents WHERE agent_id=?1)",
            [record.id.to_string()],
            |row| row.get(0),
        )
        .map_err(catalog::HostCatalogError::from)?;
    if exists {
        return Err(LocalHostError::AgentConflict(record.id));
    }
    transaction
        .execute(
            "INSERT INTO host_agents(agent_id,profile_id,name,created_by) VALUES (?1,?2,?3,?4)",
            params![
                record.id.to_string(),
                profile.as_str(),
                record.recipe.name,
                record.created_by.to_string()
            ],
        )
        .map_err(catalog::HostCatalogError::from)?;
    transaction
        .execute(
            "INSERT INTO host_bots(agent_id,profile_id,record_json) VALUES (?1,?2,?3)",
            params![record.id.to_string(), profile.as_str(), encoded],
        )
        .map_err(catalog::HostCatalogError::from)?;
    for id in &record.recipe.connections {
        let complete: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mcp_catalogs WHERE connection_id=?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(catalog::HostCatalogError::from)?;
        if !complete {
            return Err(LocalHostError::InvalidRequest(format!(
                "connection '{id}' has no complete catalog"
            )));
        }
        transaction
            .execute(
                "INSERT INTO profile_mcp_connections(profile_id,connection_id) VALUES (?1,?2)",
                params![profile.as_str(), id],
            )
            .map_err(catalog::HostCatalogError::from)?;
    }
    require_active(cancellation)?;
    transaction
        .commit()
        .map_err(catalog::HostCatalogError::from)?;
    Ok(record)
}

pub(super) fn profile(path: &Path, id: &AgentProfileId) -> Result<AgentProfile, LocalHostError> {
    let connection = catalog::open_verified(path)?;
    let record: Option<String> = connection
        .query_row(
            "SELECT record_json FROM host_bots WHERE profile_id=?1",
            [id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(catalog::HostCatalogError::from)?;
    let record: BotRecord = serde_json::from_str(&record.ok_or_else(|| {
        LocalHostError::InvalidRequest(format!(
            "agent profile `{id}` is not registered with this Host"
        ))
    })?)?;
    if profile_id(record.id)? != *id {
        return Err(LocalHostError::InvalidRequest(
            "bot profile identity mismatch".to_owned(),
        ));
    }
    let mut profile = record.recipe.profile(id)?;
    profile.selected_tools = Some(super::selection::load(&connection, &record)?.tools);
    Ok(profile)
}

pub(super) fn list(path: &Path, after: Option<AgentId>) -> Result<BotPage, LocalHostError> {
    let connection = catalog::open_verified(path)?;
    let mut statement=connection.prepare("SELECT a.agent_id,a.name,a.created_by FROM host_bots b JOIN host_agents a ON a.agent_id=b.agent_id WHERE a.agent_id>?1 ORDER BY a.agent_id LIMIT 21").map_err(catalog::HostCatalogError::from)?;
    let mut bots = statement
        .query_map(
            [after.map_or_else(String::new, |id| id.to_string())],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(catalog::HostCatalogError::from)?
        .map(|row| {
            let (id, name, creator) = row.map_err(catalog::HostCatalogError::from)?;
            Ok(BotSummary {
                id: parse_id(&id)?,
                name,
                created_by: parse_id(&creator)?,
            })
        })
        .collect::<Result<Vec<_>, LocalHostError>>()?;
    let next_cursor = if bots.len() > 20 {
        bots.truncate(20);
        bots.last().map(|bot| bot.id)
    } else {
        None
    };
    Ok(BotPage { bots, next_cursor })
}

pub(super) fn get(path: &Path, id: AgentId) -> Result<Option<BotRecord>, LocalHostError> {
    let connection = catalog::open_verified(path)?;
    let encoded: Option<String> = connection
        .query_row(
            "SELECT record_json FROM host_bots WHERE agent_id=?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(catalog::HostCatalogError::from)?;
    encoded
        .map(|encoded| {
            let record: BotRecord = serde_json::from_str(&encoded)?;
            record.recipe.validate()?;
            if record.id != id {
                return Err(LocalHostError::InvalidRequest(
                    "bot identity mismatch".to_owned(),
                ));
            }
            Ok(record)
        })
        .transpose()
}

fn parse_id(value: &str) -> Result<AgentId, LocalHostError> {
    uuid::Uuid::parse_str(value)
        .map(AgentId::from_uuid)
        .map_err(|_| LocalHostError::InvalidRequest("invalid stored bot identity".to_owned()))
}

fn require_active(cancellation: &CancellationToken) -> Result<(), LocalHostError> {
    if cancellation.is_cancelled() {
        return Err(LocalHostError::BotCreationCancelled);
    }
    Ok(())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
