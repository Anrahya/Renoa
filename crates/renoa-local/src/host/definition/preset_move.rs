//! Moving an existing agent onto a preset's current seed.

use std::path::Path;

use renoa_kernel::AgentId;
use serde::Serialize;

use super::{LocalHost, LocalHostError, catalog, catalog_error, create::commit_or_withdraw, store};
use crate::{
    AgentDefinition, AgentDefinitionError, AgentDocuments, AgentPresetId,
    documents::{AgentDocumentStore, SoulPublication},
    presets,
};

/// The outcome of moving one agent onto a preset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PresetMove {
    /// The agent's definition after the move.
    pub definition: AgentDefinition,
    /// False when the agent already followed the preset and nothing was written.
    pub changed: bool,
}

impl LocalHost {
    /// Moves one agent onto a preset's current instructions, behavior, and
    /// documents.
    ///
    /// Everything else stays: the agent's id, name, purpose, tools,
    /// connections, model, provider restriction, plugin settings, sessions,
    /// and history. The live row records the new preset id; the creation
    /// receipt keeps the definition the agent was created with. When the
    /// preset keeps a soul and the agent has no `SOUL.md`, the preset's default
    /// is published; an existing `SOUL.md` is never replaced.
    ///
    /// Validation and publication precede the commit of one row update, and a
    /// failed commit withdraws the file it published, so a failure changes
    /// nothing and a retry converges. An agent that already follows the preset
    /// is left untouched. A running node may keep serving the agent: each turn
    /// resolves the stored row, and the row names a `SOUL.md` only once that
    /// file exists, so a turn already running finishes on the old definition
    /// and the next one uses the new.
    ///
    /// # Errors
    /// Returns an error for an unknown preset or agent, an existing `SOUL.md`
    /// that cannot be read, invalid resulting state, and storage failures.
    pub async fn move_agent_to_preset(
        &self,
        agent: AgentId,
        preset_id: AgentPresetId,
    ) -> Result<PresetMove, LocalHostError> {
        let preset = presets::preset(&preset_id)?;
        let database = self.config.database.clone();
        let data_directory = self.config.home.path().to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut connection = catalog::open_verified(&database)?;
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(catalog_error)?;
            let current =
                store::read(&transaction, agent)?.ok_or(LocalHostError::AgentNotFound(agent))?;
            let mut moved = current.clone();
            moved.preset_id = Some(preset_id);
            moved.operational.instructions = preset.instructions(None);
            moved.operational.behavior = preset.behavior();
            moved.operational.documents = preset.documents();
            if moved == current {
                return Ok(PresetMove {
                    definition: current,
                    changed: false,
                });
            }
            moved.validate()?;
            store::update_root(&transaction, &moved)?;
            // Publication is the last step before the commit, as in creation.
            let publication = match moved.operational.documents {
                Some(enabled) => keep_or_publish_soul(
                    &data_directory,
                    agent,
                    enabled,
                    preset.soul_default().unwrap_or_default(),
                )?,
                None => None,
            };
            #[cfg(test)]
            super::commit_fault::inject(&transaction, agent).map_err(catalog_error)?;
            commit_or_withdraw(transaction, publication)?;
            Ok(PresetMove {
                definition: moved,
                changed: true,
            })
        })
        .await?
    }
}

/// Publishes the preset's default `SOUL.md` unless the agent already keeps one,
/// then proves the agent's documents open as the next turn will open them.
fn keep_or_publish_soul(
    data_directory: &Path,
    agent: AgentId,
    enabled: AgentDocuments,
    soul: &'static str,
) -> Result<Option<SoulPublication>, LocalHostError> {
    let publication = match AgentDocumentStore::publish(data_directory, agent, enabled, soul) {
        // The agent's own SOUL.md stays as it is.
        Err(AgentDefinitionError::DocumentConflict { .. }) => None,
        published => published?,
    };
    if let Err(error) = AgentDocumentStore::open(data_directory, agent, enabled) {
        if let Some(publication) = publication {
            publication.withdraw(error.to_string())?;
        }
        return Err(error.into());
    }
    Ok(publication)
}
