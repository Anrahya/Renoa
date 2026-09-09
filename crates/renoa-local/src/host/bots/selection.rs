//! Mutable tool selection, separate from immutable bot creation receipts.
use std::collections::BTreeSet;

use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{BotRecord, LocalHost, LocalHostError};
use crate::host::catalog;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotToolSelection {
    pub id: AgentId,
    pub revision: i64,
    pub tools: BTreeSet<String>,
}

/// Trusted local management input; remote callers must authenticate as the owner.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotToolsUpdate {
    pub operation_id: Uuid,
    pub id: AgentId,
    pub expected_revision: i64,
    pub tools: BTreeSet<String>,
}

pub(in crate::host) fn initialize(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(), catalog::HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_bot_tool_selections (
        agent_id TEXT PRIMARY KEY REFERENCES host_bots(agent_id),
        revision INTEGER NOT NULL CHECK(revision > 0),
        tools_json TEXT NOT NULL CHECK(json_valid(tools_json))
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_bot_tool_operations (
        operation_id TEXT PRIMARY KEY, request_json TEXT NOT NULL, result_json TEXT NOT NULL
    ) STRICT;",
    )?;
    Ok(())
}

pub(super) fn load(
    db: &rusqlite::Connection,
    record: &BotRecord,
) -> Result<BotToolSelection, LocalHostError> {
    let stored: Option<(i64, String)> = db
        .query_row(
            "SELECT revision,tools_json FROM host_bot_tool_selections WHERE agent_id=?1",
            [record.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(catalog::HostCatalogError::from)?;
    let (revision, tools) = match stored {
        Some((revision, tools)) => (revision, serde_json::from_str(&tools)?),
        None => (0, record.recipe.tools.clone()),
    };
    Ok(BotToolSelection {
        id: record.id,
        revision,
        tools,
    })
}

impl LocalHost {
    /// Reads the effective selection; revision zero is the immutable creation recipe.
    /// # Errors
    /// Rejects missing bots or corrupt storage without loading a model.
    pub async fn bot_tool_selection(
        &self,
        id: AgentId,
    ) -> Result<BotToolSelection, LocalHostError> {
        let record = self
            .bot(id)
            .await?
            .ok_or(LocalHostError::AgentNotFound(id))?;
        let path = self.config.database.clone();
        tokio::task::spawn_blocking(move || load(&catalog::open_verified(&path)?, &record)).await?
    }

    /// Applies an owner-authorized tool edit with revision checks and exact replay.
    /// Existing runs retain their frozen tool bindings. Creation receipts are unchanged.
    /// # Errors
    /// Rejects unsupported tools, unknown bots, stale revisions and conflicting retries.
    pub async fn configure_bot_tools(
        &self,
        edit: BotToolsUpdate,
    ) -> Result<BotToolSelection, LocalHostError> {
        let record = self
            .bot(edit.id)
            .await?
            .ok_or(LocalHostError::AgentNotFound(edit.id))?;
        let mut recipe = record.recipe.clone();
        recipe.tools.clone_from(&edit.tools);
        recipe.validate()?;
        let path = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let mut db = catalog::open_verified(&path)?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(catalog::HostCatalogError::from)?;
            let request = serde_json::to_string(&edit)?;
            let receipt: Option<(String,String)> = tx.query_row(
                "SELECT request_json,result_json FROM host_bot_tool_operations WHERE operation_id=?1",
                [edit.operation_id.to_string()], |row| Ok((row.get(0)?,row.get(1)?)))
                .optional().map_err(catalog::HostCatalogError::from)?;
            if let Some((previous, result)) = receipt {
                if previous != request { return Err(LocalHostError::AgentConflict(edit.id)); }
                return Ok(serde_json::from_str(&result)?);
            }
            let current = load(&tx, &record)?;
            if current.revision != edit.expected_revision { return Err(LocalHostError::AgentConflict(edit.id)); }
            let result = BotToolSelection { id: edit.id, revision: current.revision.checked_add(1)
                .ok_or_else(|| LocalHostError::InvalidRequest("tool selection revision exhausted".to_owned()))?, tools: edit.tools };
            tx.execute("INSERT INTO host_bot_tool_selections VALUES(?1,?2,?3)
                ON CONFLICT(agent_id) DO UPDATE SET revision=excluded.revision,tools_json=excluded.tools_json",
                params![edit.id.to_string(), result.revision, serde_json::to_string(&result.tools)?]).map_err(catalog::HostCatalogError::from)?;
            tx.execute("INSERT INTO host_bot_tool_operations VALUES(?1,?2,?3)",
                params![edit.operation_id.to_string(), request, serde_json::to_string(&result)?]).map_err(catalog::HostCatalogError::from)?;
            tx.commit().map_err(catalog::HostCatalogError::from)?;
            Ok(result)
        }).await?
    }
}

#[cfg(test)]
mod tests;
