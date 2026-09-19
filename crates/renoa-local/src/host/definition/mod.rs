//! The canonical Host-owned agent definition: storage API and reads.
//!
//! Creation itself lives in [`create`], which owns validation, identity
//! derivation, document publication, and the single creation transaction.

use std::collections::BTreeSet;

use renoa_kernel::AgentId;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{LocalHost, LocalHostError, catalog};
use crate::{
    AgentDefinition, AgentPresetId, AgentToolSelection, capabilities, stable_id::stable_id,
};

mod create;
pub(in crate::host) mod schema;
mod store;

#[cfg(test)]
mod tests;

/// The largest page a caller may request when listing agents.
pub(in crate::host) const MAX_AGENT_PAGE: usize = 20;

const AGENT_ID_DOMAIN: &str = "renoa.agent.create.v1";

/// The optional first routine created with a new agent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentRoutine {
    pub name: String,
    pub prompt: String,
    pub schedule: super::routines::RoutineSchedule,
    pub enabled: bool,
}

/// The canonical creation request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentCreateRequest {
    pub operation_id: Uuid,
    pub preset_id: AgentPresetId,
    pub name: String,
    /// Required by presets that take caller instructions, rejected by fixed ones.
    pub instructions: Option<String>,
    /// Exact caller-selected capability names.
    pub tools: BTreeSet<String>,
    /// Exact caller-selected existing Host connection ids.
    pub connections: BTreeSet<String>,
    pub routine: Option<AgentRoutine>,
}

impl AgentCreateRequest {
    /// Starts a request for one preset.
    #[must_use]
    pub fn new(operation_id: Uuid, preset_id: AgentPresetId, name: impl Into<String>) -> Self {
        Self {
            operation_id,
            preset_id,
            name: name.into(),
            instructions: None,
            tools: BTreeSet::new(),
            connections: BTreeSet::new(),
            routine: None,
        }
    }

    /// Supplies instructions for presets that accept them.
    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Adds exact capability names to the caller selection.
    #[must_use]
    pub fn with_tools(mut self, tools: impl IntoIterator<Item = String>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// Attaches an optional first routine.
    #[must_use]
    pub fn with_routine(mut self, routine: AgentRoutine) -> Self {
        self.routine = Some(routine);
        self
    }
}

/// One revision-checked capability edit.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentToolsUpdate {
    pub operation_id: Uuid,
    pub id: AgentId,
    pub expected_revision: i64,
    pub tools: BTreeSet<String>,
}

/// One revision-checked display-name edit.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RenameAgent {
    pub id: AgentId,
    pub expected_name: String,
    pub name: String,
}

/// Derives the stable agent identity of one creation operation.
///
/// Every caller uses this helper so the hash algorithm cannot drift.
#[must_use]
pub fn derived_agent_id(operation_id: Uuid) -> AgentId {
    AgentId::from_uuid(stable_id(&format!("{AGENT_ID_DOMAIN}:{operation_id}")))
}

impl LocalHost {
    /// Reads one canonical agent definition.
    ///
    /// # Errors
    /// Returns catalog storage or definition corruption errors.
    pub async fn agent_definition(
        &self,
        id: AgentId,
    ) -> Result<Option<AgentDefinition>, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let connection = catalog::open_verified(&database)?;
            Ok(store::read(&connection, id)?)
        })
        .await?
    }

    /// Lists canonical agent definitions in identity order.
    ///
    /// # Errors
    /// Returns invalid page sizes, catalog storage, or definition errors.
    pub async fn list_agent_definitions(
        &self,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<Vec<AgentDefinition>, LocalHostError> {
        if limit == 0 || limit > MAX_AGENT_PAGE {
            return Err(LocalHostError::InvalidRequest(format!(
                "agent page size must be 1-{MAX_AGENT_PAGE}"
            )));
        }
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let connection = catalog::open_verified(&database)?;
            Ok(store::list(&connection, after, limit)?)
        })
        .await?
    }

    /// Applies an owner-authorized capability edit with revision checks and
    /// exact replay.
    ///
    /// # Errors
    /// Rejects unknown capabilities or agents, stale revisions, and conflicting
    /// retries.
    pub async fn set_agent_tools(
        &self,
        update: AgentToolsUpdate,
    ) -> Result<AgentToolSelection, LocalHostError> {
        for name in &update.tools {
            require_selectable(name)?;
        }
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = catalog::open_verified(&database)?;
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(catalog_error)?;
            let request_json = serde_json::to_string(&update)?;
            if let Some(receipt) = store::selection_receipt(&transaction, update.operation_id)? {
                if receipt.request_json != request_json {
                    return Err(LocalHostError::AgentConflict(update.id));
                }
                return Ok(serde_json::from_str(&receipt.result_json)?);
            }
            let current = store::read_selection(&transaction, update.id)
                .map_err(|_| LocalHostError::AgentNotFound(update.id))?;
            if current.revision != update.expected_revision {
                return Err(LocalHostError::AgentConflict(update.id));
            }
            let selection = AgentToolSelection {
                revision: current.revision.checked_add(1).ok_or_else(|| {
                    LocalHostError::InvalidRequest(
                        "agent tool selection revision exhausted".to_owned(),
                    )
                })?,
                tools: update.tools,
            };
            store::write_selection(&transaction, update.id, &selection)?;
            let result_json = serde_json::to_string(&selection)?;
            store::insert_selection_receipt(
                &transaction,
                update.operation_id,
                &request_json,
                &result_json,
            )?;
            transaction.commit().map_err(catalog_error)?;
            Ok(selection)
        })
        .await?
    }

    /// Renames one agent. An agent may rename itself, and an agent whose
    /// selection contains the management capability may rename another agent.
    ///
    /// # Errors
    /// Rejects unauthorized actors, stale names, conflicting replays, and
    /// storage failures.
    pub async fn rename_agent(
        &self,
        actor: AgentId,
        operation: Uuid,
        edit: RenameAgent,
        cancellation: CancellationToken,
    ) -> Result<AgentDefinition, LocalHostError> {
        if edit.name.trim().is_empty()
            || edit.name.len() > crate::agent_definition::MAX_NAME_BYTES
            || edit.name.trim() != edit.name
        {
            return Err(LocalHostError::InvalidRequest(
                "agent name must contain 1-512 bytes without leading or trailing whitespace"
                    .to_owned(),
            ));
        }
        if operation.is_nil() {
            return Err(LocalHostError::InvalidRequest(
                "agent rename requires a non-nil operation id".to_owned(),
            ));
        }
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = catalog::open_verified(&database)?;
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(catalog_error)?;
            let request_json = serde_json::to_string(&edit)?;
            if let Some((stored_actor, request, result)) =
                store::rename_receipt(&transaction, operation)?
            {
                if stored_actor != actor.to_string() || request != request_json {
                    return Err(LocalHostError::AgentConflict(edit.id));
                }
                return Ok(serde_json::from_str(&result)?);
            }
            let mut definition = store::read(&transaction, edit.id)?
                .ok_or(LocalHostError::AgentNotFound(edit.id))?;
            if definition.name != edit.expected_name {
                return Err(LocalHostError::AgentConflict(edit.id));
            }
            if actor != edit.id {
                let actor = store::read(&transaction, actor)?
                    .ok_or(LocalHostError::AgentNotFound(actor))?;
                if !actor
                    .tool_selection
                    .tools
                    .contains(capabilities::AGENT_MANAGE)
                {
                    return Err(LocalHostError::InvalidRequest(format!(
                        "agent `{actor:?}` has no `{}` capability",
                        capabilities::AGENT_MANAGE
                    )));
                }
            }
            check_cancellation(&cancellation)?;
            definition.name.clone_from(&edit.name);
            definition.validate()?;
            store::set_name(&transaction, edit.id, &definition.name)?;
            let result_json = serde_json::to_string(&definition)?;
            store::insert_rename_receipt(
                &transaction,
                operation,
                edit.id,
                actor,
                &request_json,
                &result_json,
            )?;
            transaction.commit().map_err(catalog_error)?;
            Ok(definition)
        })
        .await?
    }
}

pub(in crate::host) fn require_selectable(name: &str) -> Result<(), LocalHostError> {
    if capabilities::is_selectable(name) {
        Ok(())
    } else {
        Err(LocalHostError::InvalidRequest(format!(
            "`{name}` is not a Host capability"
        )))
    }
}

pub(in crate::host) fn check_cancellation(
    cancellation: &CancellationToken,
) -> Result<(), LocalHostError> {
    if cancellation.is_cancelled() {
        Err(LocalHostError::AgentCancelled)
    } else {
        Ok(())
    }
}

pub(in crate::host) fn catalog_error(error: rusqlite::Error) -> LocalHostError {
    catalog::HostCatalogError::from(error).into()
}
