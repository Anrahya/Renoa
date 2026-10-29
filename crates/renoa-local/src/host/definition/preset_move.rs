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
    /// False when nothing was written: the agent already followed the preset
    /// and kept a readable `SOUL.md` wherever the preset needs one.
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
    /// is published; an existing `SOUL.md` is never replaced. An agent already
    /// on the preset keeps its row, and only a missing `SOUL.md` is restored.
    ///
    /// It refuses to run as anyone but the owner of the Host data directory,
    /// whose files the Host must be able to read. Validation and publication
    /// precede the commit of one row update, and a failed commit withdraws the
    /// file it published and the directories it made, so a retry converges.
    /// Publishing into an existing agent directory first restricts it to its
    /// owner, and that permission change stays. A running node may keep serving
    /// the agent: each turn resolves the stored row, and the row names a
    /// `SOUL.md` only once that file exists, so a turn already running finishes
    /// on the old definition and the next one uses the new.
    ///
    /// # Errors
    /// Returns an error for an unknown preset or agent, a caller that does not
    /// own the data directory, an existing `SOUL.md` that cannot be read,
    /// invalid resulting state, and storage failures.
    pub async fn move_agent_to_preset(
        &self,
        agent: AgentId,
        preset_id: AgentPresetId,
    ) -> Result<PresetMove, LocalHostError> {
        let seed = presets::preset(&preset_id)?.seed();
        let database = self.config.database.clone();
        let data_directory = self.config.home.path().to_path_buf();
        tokio::task::spawn_blocking(move || {
            require_data_owner(&data_directory)?;
            let mut connection = catalog::open_verified(&database)?;
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(catalog_error)?;
            let current =
                store::read(&transaction, agent)?.ok_or(LocalHostError::AgentNotFound(agent))?;
            let mut moved = current.clone();
            moved.preset_id = Some(preset_id);
            seed.instructions
                .clone_into(&mut moved.operational.instructions);
            moved.operational.behavior = seed.behavior;
            moved.operational.documents = seed.documents;
            let unchanged = moved == current;
            if !unchanged {
                moved.validate()?;
                store::update_root(&transaction, &moved)?;
            }
            // Publication is the last step before the commit, as in creation.
            let publication = match seed.documents {
                Some(enabled) => keep_or_publish_soul(
                    &data_directory,
                    agent,
                    enabled,
                    seed.soul.unwrap_or_default(),
                )?,
                None => None,
            };
            if unchanged {
                // No row to commit: a restored SOUL.md is the only change.
                return Ok(PresetMove {
                    definition: current,
                    changed: publication.as_ref().is_some_and(SoulPublication::created),
                });
            }
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
        return Err(match publication {
            Some(publication) => publication.withdraw(error)?.into(),
            None => error.into(),
        });
    }
    Ok(publication)
}

/// Refuses a caller other than the data directory's owner, before any effect:
/// a file it published would be unreadable to the Host.
fn require_data_owner(data_directory: &Path) -> Result<(), LocalHostError> {
    use std::os::unix::fs::MetadataExt as _;

    let owner = std::fs::metadata(data_directory)
        .map_err(|error| {
            LocalHostError::InvalidRequest(format!(
                "cannot inspect the Host data directory {}: {error}",
                data_directory.display()
            ))
        })?
        .uid();
    #[cfg(test)]
    let owner = owner_fault::owner(data_directory).unwrap_or(owner);
    let effective = nix::unistd::geteuid();
    if owner == effective.as_raw() {
        return Ok(());
    }
    let owner_name = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(owner))
        .ok()
        .flatten()
        .map_or_else(|| format!("uid {owner}"), |user| user.name);
    Err(LocalHostError::InvalidRequest(format!(
        "run this as {owner_name}, the owner of the Host data directory {}; this process runs as uid {effective}, and the Host could not read files it writes",
        data_directory.display()
    )))
}

/// Pretends a chosen data directory has another owner, so the refusal is
/// testable without a second user.
#[cfg(test)]
pub(super) mod owner_fault {
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::Mutex,
    };

    static OWNERS: Mutex<BTreeMap<PathBuf, u32>> = Mutex::new(BTreeMap::new());

    pub(in crate::host) fn pretend(data_directory: &Path, owner: u32) {
        let directory = std::fs::canonicalize(data_directory).expect("data directory");
        OWNERS.lock().expect("owner lock").insert(directory, owner);
    }

    pub(super) fn owner(data_directory: &Path) -> Option<u32> {
        let directory = std::fs::canonicalize(data_directory).ok()?;
        OWNERS.lock().expect("owner lock").get(&directory).copied()
    }
}
