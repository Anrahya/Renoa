use std::collections::BTreeSet;

use renoa_kernel::AgentId;
use serde::{Deserialize, Serialize};

use super::{LocalHost, LocalHostError};
use crate::{AgentProfile, AgentProfileId};

pub(crate) mod names;
mod store;
#[cfg(test)]
mod tests;
pub(crate) mod tool;

/// A persisted specialist recipe. Connections refer to existing Host accounts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotRecipe {
    pub name: String,
    pub instructions: String,
    pub tools: BTreeSet<String>,
    pub connections: BTreeSet<String>,
}

impl BotRecipe {
    fn validate(&self) -> Result<(), LocalHostError> {
        if self.name.trim().is_empty()
            || self.name.len() > 512
            || self.instructions.trim().is_empty()
            || self.instructions.len() > 32 * 1024
            || self.connections.len() > 64
            || self
                .connections
                .iter()
                .any(|id| id.is_empty() || id.len() > 256)
            || self.tools.iter().any(|tool| {
                !matches!(
                    tool.as_str(),
                    "read_file"
                        | "write_file"
                        | "edit_file"
                        | "bash"
                        | "grep"
                        | "find"
                        | "extension_manage"
                        | "bot_manage"
                )
            })
        {
            return Err(LocalHostError::InvalidRequest("invalid bot recipe: provide a nonblank name and instructions, supported tool names, and existing Host connections".to_owned()));
        }
        Ok(())
    }

    fn profile(&self, id: &AgentProfileId) -> Result<AgentProfile, LocalHostError> {
        self.validate()?;
        let mut profile = AgentProfile::new(id.as_str(), &self.instructions)?.with_turn_timing();
        profile.selected_tools = Some(self.tools.clone());
        Ok(profile)
    }
}

/// The creation recipe and durable Agent identity of a specialist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotRecord {
    pub id: AgentId,
    pub created_by: AgentId,
    pub recipe: BotRecipe,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotSummary {
    pub id: AgentId,
    pub name: String,
    pub created_by: AgentId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BotPage {
    pub bots: Vec<BotSummary>,
    pub next_cursor: Option<AgentId>,
}

impl LocalHost {
    /// Creates a specialist and attaches existing connections atomically.
    /// Repeating identical creation is idempotent; changed fields conflict.
    ///
    /// # Errors
    /// Rejects unknown creators/connections, invalid recipes, conflicts, or storage errors.
    pub async fn ensure_bot(&self, record: BotRecord) -> Result<BotRecord, LocalHostError> {
        self.ensure_bot_with_cancellation(record, tokio_util::sync::CancellationToken::new())
            .await
    }

    pub(crate) async fn ensure_bot_with_cancellation(
        &self,
        record: BotRecord,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<BotRecord, LocalHostError> {
        record.recipe.validate()?;
        if self.agent(record.created_by).await?.is_none() {
            return Err(LocalHostError::AgentNotFound(record.created_by));
        }
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || store::ensure(&database, record, &cancellation)).await?
    }

    /// Lists persisted specialists without loading a model or opening their sessions.
    ///
    /// # Errors
    /// Returns corrupt recipe or catalog storage errors.
    pub async fn list_bots(&self, after: Option<AgentId>) -> Result<BotPage, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || store::list(&database, after)).await?
    }

    /// Reads one specialist recipe without opening an executable session.
    ///
    /// # Errors
    /// Returns invalid stored data or catalog storage errors.
    pub async fn bot(&self, id: AgentId) -> Result<Option<BotRecord>, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || store::get(&database, id)).await?
    }

    /// Opens a specialist's Host-owned workspace, separate from the operator's workspace.
    ///
    /// # Errors
    /// Returns an unknown bot or filesystem error.
    pub async fn bot_workspace(&self, id: AgentId) -> Result<std::path::PathBuf, LocalHostError> {
        if self.bot(id).await?.is_none() {
            return Err(LocalHostError::AgentNotFound(id));
        }
        let root = self
            .config
            .sessions
            .parent()
            .ok_or_else(|| {
                LocalHostError::InvalidRequest("Host sessions have no data root".to_owned())
            })?
            .join("bot-workspaces");
        tokio::task::spawn_blocking(move || {
            let path = root.join(id.to_string());
            std::fs::create_dir_all(&path)?;
            Ok(std::fs::canonicalize(path)?)
        })
        .await?
    }
}

pub(crate) async fn resolve_profile(
    host: &super::HostConfig,
    id: &AgentProfileId,
) -> Result<AgentProfile, LocalHostError> {
    if let Some(profile) = host.profiles.get(id) {
        return Ok(profile.clone());
    }
    let database = host.database.clone();
    let id = id.clone();
    tokio::task::spawn_blocking(move || store::profile(&database, &id)).await?
}

pub(super) fn profile_id(id: AgentId) -> Result<AgentProfileId, LocalHostError> {
    Ok(AgentProfileId::new(format!("renoa.bot.{id}"))?)
}
