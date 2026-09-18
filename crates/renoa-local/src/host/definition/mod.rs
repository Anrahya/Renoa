//! The canonical Host-owned agent definition and the one creation operation.
//!
//! Creation is used by trusted provisioning, the agent-facing management tool,
//! and (later) the authenticated management API. It validates identically for
//! every caller, derives a stable agent id from the operation id, publishes
//! documents before the database commit, and writes identity, the explicit tool
//! selection, connections, an optional first routine, and its typed receipt in
//! one transaction.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use renoa_kernel::AgentId;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{HostConfig, LocalHost, LocalHostError, catalog, routines};
use crate::{
    AgentCreationOrigin, AgentCreator, AgentDefinition, AgentDocuments, AgentOperationalDefinition,
    AgentPresetId, AgentToolSelection, capabilities, documents::DocumentDefaults, presets,
};

pub(in crate::host) mod schema;
mod store;

#[cfg(test)]
mod tests;

/// The largest page a caller may request when listing agents.
pub(in crate::host) const MAX_AGENT_PAGE: usize = 20;

const AGENT_ID_DOMAIN: &str = "renoa.agent.create.v1";
const ROUTINE_ID_DOMAIN: &str = "renoa.agent.routine.v1";

/// The optional first routine created with a new agent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentRoutine {
    pub name: String,
    pub prompt: String,
    pub schedule: routines::RoutineSchedule,
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

    /// Adds existing Host connection ids.
    #[must_use]
    pub fn with_connections(mut self, connections: impl IntoIterator<Item = String>) -> Self {
        self.connections.extend(connections);
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
    AgentId::from_uuid(derived_uuid(AGENT_ID_DOMAIN, operation_id))
}

fn derived_routine_id(operation_id: Uuid) -> Uuid {
    derived_uuid(ROUTINE_ID_DOMAIN, operation_id)
}

fn derived_uuid(domain: &str, operation_id: Uuid) -> Uuid {
    let digest = Sha256::digest(format!("{domain}:{operation_id}"));
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

impl LocalHost {
    /// Creates one durable agent, or replays the stored result of the same
    /// operation.
    ///
    /// # Errors
    /// Rejects an untrusted actor/origin pairing, an unknown preset, invalid
    /// fields, unknown capabilities or connections, conflicting replays, and
    /// storage failures.
    pub async fn create_agent(
        &self,
        creator: AgentCreator,
        origin: AgentCreationOrigin,
        request: AgentCreateRequest,
        cancellation: CancellationToken,
    ) -> Result<AgentDefinition, LocalHostError> {
        validate_actor(&creator, origin)?;
        if request.operation_id.is_nil() {
            return Err(LocalHostError::InvalidRequest(
                "agent creation requires a non-nil operation id".to_owned(),
            ));
        }
        let preset = presets::preset(&request.preset_id)?;
        let instructions = preset.instructions(request.instructions.as_deref())?;
        let selection = resolve_selection(preset.tool_baseline(), &request.tools)?;
        let definition = AgentDefinition {
            id: derived_agent_id(request.operation_id),
            name: request.name.clone(),
            created_at_ms: host_now_ms()?,
            creator: creator.clone(),
            created_via: origin,
            preset_id: Some(preset.id().clone()),
            operational: AgentOperationalDefinition {
                instructions,
                behavior: preset.behavior(),
                documents: preset.documents(),
                provider_restriction: preset.provider_restriction(),
            },
            tool_selection: AgentToolSelection {
                revision: 1,
                tools: selection,
            },
            connections: request.connections.clone(),
        };
        definition.validate()?;
        let document_defaults = preset.documents().zip(preset.document_defaults());
        if let Some((enabled, _)) = document_defaults
            && !enabled.any()
        {
            return Err(LocalHostError::InvalidRequest(
                "an agent cannot publish an empty document set".to_owned(),
            ));
        }
        let database = self.config.database.clone();
        let data_directory = data_directory(&self.config)?;
        let request_json = serde_json::to_string(&request)?;
        let routine = request.routine.map(|routine| routines::RoutineSpec {
            agent_id: definition.id,
            name: routine.name,
            prompt: routine.prompt,
            schedule: routine.schedule,
            enabled: routine.enabled,
        });
        let routine_id = derived_routine_id(request.operation_id);
        tokio::task::spawn_blocking(move || {
            create_blocking(
                &database,
                &data_directory,
                &definition,
                request.operation_id,
                &request_json,
                document_defaults,
                routine.map(|spec| (routine_id, spec)),
                &cancellation,
            )
        })
        .await?
    }

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
            let transaction = connection.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )
            .map_err(catalog_err)?;
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
                revision: current
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| {
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
            transaction.commit().map_err(catalog_err)?;
            Ok(selection)
        })
        .await?
    }

    /// Reads one agent's effective tool selection.
    ///
    /// # Errors
    /// Returns catalog storage errors, including a missing selection row.
    pub async fn agent_tool_selection(
        &self,
        id: AgentId,
    ) -> Result<AgentToolSelection, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let connection = catalog::open_verified(&database)?;
            Ok(store::read_selection(&connection, id)?)
        })
        .await?
    }

    /// Reads one agent's selected Host connections.
    ///
    /// # Errors
    /// Returns catalog storage errors.
    pub async fn agent_connections(
        &self,
        id: AgentId,
    ) -> Result<BTreeSet<String>, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let connection = catalog::open_verified(&database)?;
            Ok(store::read_connections(&connection, id)?)
        })
        .await?
    }

    /// Adds one existing Host connection to an agent.
    ///
    /// # Errors
    /// Rejects unknown agents or connections and storage failures.
    pub async fn enable_agent_connection(
        &self,
        id: AgentId,
        connection_id: &str,
    ) -> Result<BTreeSet<String>, LocalHostError> {
        if connection_id.is_empty() {
            return Err(LocalHostError::InvalidRequest(
                "connection id must not be empty".to_owned(),
            ));
        }
        let database = self.config.database.clone();
        let connection_id = connection_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut connection = catalog::open_verified(&database)?;
            let transaction = connection.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )
            .map_err(catalog_err)?;
            if !store::exists(&transaction, id)? {
                return Err(LocalHostError::AgentNotFound(id));
            }
            let mut selected = store::read_connections(&transaction, id)?;
            selected.insert(connection_id);
            store::set_connections(&transaction, id, &selected)?;
            transaction.commit().map_err(catalog_err)?;
            Ok(selected)
        })
        .await?
    }

    /// Removes one Host connection from an agent.
    ///
    /// # Errors
    /// Rejects unknown agents and storage failures.
    pub async fn disable_agent_connection(
        &self,
        id: AgentId,
        connection_id: &str,
    ) -> Result<BTreeSet<String>, LocalHostError> {
        let database = self.config.database.clone();
        let connection_id = connection_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut connection = catalog::open_verified(&database)?;
            let transaction = connection.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )
            .map_err(catalog_err)?;
            if !store::exists(&transaction, id)? {
                return Err(LocalHostError::AgentNotFound(id));
            }
            let mut selected = store::read_connections(&transaction, id)?;
            selected.remove(&connection_id);
            store::set_connections(&transaction, id, &selected)?;
            transaction.commit().map_err(catalog_err)?;
            Ok(selected)
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
            let transaction = connection.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )
            .map_err(catalog_err)?;
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
            active(&cancellation)?;
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
            transaction.commit().map_err(catalog_err)?;
            Ok(definition)
        })
        .await?
    }
}

fn create_blocking(
    database: &std::path::Path,
    data_directory: &std::path::Path,
    definition: &AgentDefinition,
    operation_id: Uuid,
    request_json: &str,
    document_defaults: Option<(AgentDocuments, DocumentDefaults)>,
    routine: Option<(Uuid, routines::RoutineSpec)>,
    cancellation: &CancellationToken,
) -> Result<AgentDefinition, LocalHostError> {
    let mut connection = catalog::open_verified(database)?;
    if let Some(existing) = replay(&connection, operation_id, definition, request_json)? {
        return Ok(existing);
    }
    active(cancellation)?;
    if let Some((enabled, defaults)) = document_defaults {
        crate::documents::AgentDocumentStore::publish(
            data_directory,
            definition.id,
            enabled,
            defaults,
        )?;
    }
    active(cancellation)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(catalog_err)?;
    // The write lock is held: a concurrent create with the same operation id
    // has either committed or not yet started, so this read is authoritative.
    if let Some(existing) = replay(&transaction, operation_id, definition, request_json)? {
        return Ok(existing);
    }
    store::insert(&transaction, definition)?;
    if let Some((routine_id, spec)) = routine {
        routines::store::insert_first_routine(
            &transaction,
            routine_id,
            spec,
            definition.created_at_ms,
        )?;
    }
    let receipt = store::CreationReceipt {
        agent_id: definition.id,
        created_via: definition.created_via,
        creator: definition.creator.clone(),
        request_json: request_json.to_owned(),
        result_json: serde_json::to_string(definition)?,
    };
    store::insert_creation_receipt(&transaction, operation_id, &receipt)?;
    active(cancellation)?;
    transaction.commit().map_err(catalog_err)?;
    Ok(definition.clone())
}

fn replay(
    connection: &rusqlite::Connection,
    operation_id: Uuid,
    definition: &AgentDefinition,
    request_json: &str,
) -> Result<Option<AgentDefinition>, LocalHostError> {
    let Some(receipt) = store::creation_receipt(connection, operation_id)? else {
        return Ok(None);
    };
    if receipt.creator != definition.creator
        || receipt.created_via != definition.created_via
        || receipt.request_json != request_json
    {
        return Err(LocalHostError::AgentConflict(receipt.agent_id));
    }
    Ok(Some(serde_json::from_str(&receipt.result_json)?))
}

fn validate_actor(
    creator: &AgentCreator,
    origin: AgentCreationOrigin,
) -> Result<(), LocalHostError> {
    let trusted = matches!(
        (origin, creator),
        (AgentCreationOrigin::AgentTool, AgentCreator::Agent { .. })
            | (AgentCreationOrigin::Management, AgentCreator::Principal { .. })
            | (AgentCreationOrigin::Provisioning, AgentCreator::System { .. })
    );
    if trusted {
        Ok(())
    } else {
        Err(LocalHostError::InvalidRequest(
            "agent creation origin and creator do not match a trusted caller".to_owned(),
        ))
    }
}

fn resolve_selection(
    baseline: capabilities::PresetToolBaseline,
    caller: &BTreeSet<String>,
) -> Result<BTreeSet<String>, LocalHostError> {
    for name in caller {
        require_selectable(name)?;
    }
    Ok(capabilities::baseline_selection(baseline, caller))
}

fn require_selectable(name: &str) -> Result<(), LocalHostError> {
    if capabilities::is_selectable(name) {
        Ok(())
    } else {
        Err(LocalHostError::InvalidRequest(format!(
            "`{name}` is not a Host capability"
        )))
    }
}

fn active(cancellation: &CancellationToken) -> Result<(), LocalHostError> {
    if cancellation.is_cancelled() {
        Err(LocalHostError::AgentCancelled)
    } else {
        Ok(())
    }
}

fn catalog_err(error: rusqlite::Error) -> LocalHostError {
    catalog::HostCatalogError::from(error).into()
}

fn data_directory(config: &HostConfig) -> Result<std::path::PathBuf, LocalHostError> {
    config
        .sessions
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| {
            LocalHostError::InvalidRequest("Host sessions have no data root".to_owned())
        })
}

fn host_now_ms() -> Result<i64, LocalHostError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| LocalHostError::InvalidRequest(format!("Host clock error: {error}")))?;
    i64::try_from(elapsed.as_millis()).map_err(|_| {
        LocalHostError::InvalidRequest("Host clock exceeds the supported range".to_owned())
    })
}
