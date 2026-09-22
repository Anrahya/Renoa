//! The one canonical agent creation operation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    AgentCreateRequest, LocalHost, LocalHostError, catalog_error, check_cancellation,
    require_consumable, require_selectable, store,
};
use crate::{
    AgentCreationOrigin, AgentCreator, AgentDefinition, AgentDocuments, AgentOperationalDefinition,
    AgentToolSelection, capabilities,
    documents::DocumentDefaults,
    host::{HostConfig, catalog, routines},
    presets,
    stable_id::stable_id,
};

const ROUTINE_ID_DOMAIN: &str = "renoa.agent.routine.v1";

impl LocalHost {
    /// Creates one durable agent, or replays the stored result of the same
    /// operation.
    ///
    /// A replay returns the definition this operation committed. Later edits
    /// change the live agent but never that stored result.
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
        let preset = presets::preset(&request.preset_id)?;
        let definition = AgentDefinition {
            id: super::derived_agent_id(request.operation_id),
            name: request.name.clone(),
            created_at_ms: host_now_ms()?,
            creator,
            created_via: origin,
            preset_id: Some(preset.id().clone()),
            operational: AgentOperationalDefinition {
                instructions: preset.instructions(request.instructions.as_deref())?,
                behavior: preset.behavior(),
                documents: preset.documents(),
                provider_restriction: preset.provider_restriction(),
            },
            tool_selection: AgentToolSelection {
                revision: 1,
                tools: resolve_selection(
                    preset.capability_baseline(),
                    preset.documents(),
                    &request.tools,
                )?,
            },
            connections: request.connections.clone(),
        };
        definition.validate()?;
        if request.operation_id.is_nil() {
            return Err(LocalHostError::InvalidRequest(
                "agent creation requires a non-nil operation id".to_owned(),
            ));
        }
        let request_json = serde_json::to_string(&request)?;
        let result_json = serde_json::to_string(&definition)?;
        let routine = request.routine.map(|routine| {
            (
                stable_id(&format!("{ROUTINE_ID_DOMAIN}:{}", request.operation_id)),
                routines::RoutineSpec {
                    agent_id: definition.id,
                    name: routine.name,
                    prompt: routine.prompt,
                    schedule: routine.schedule,
                    enabled: routine.enabled,
                },
            )
        });
        let commit = CreateCommit {
            database: self.config.database.clone(),
            data_directory: data_directory(&self.config)?,
            definition,
            operation_id: request.operation_id,
            request_json,
            result_json,
            document_defaults: preset.documents().zip(preset.document_defaults()),
            routine,
            cancellation,
        };
        tokio::task::spawn_blocking(move || create_blocking(&commit)).await?
    }
}

/// Everything the blocking creation unit needs, named so the two adjacent
/// paths cannot be transposed at the call site.
struct CreateCommit {
    database: PathBuf,
    data_directory: PathBuf,
    definition: AgentDefinition,
    operation_id: Uuid,
    request_json: String,
    result_json: String,
    document_defaults: Option<(AgentDocuments, DocumentDefaults)>,
    routine: Option<(Uuid, routines::RoutineSpec)>,
    cancellation: CancellationToken,
}

fn create_blocking(commit: &CreateCommit) -> Result<AgentDefinition, LocalHostError> {
    let CreateCommit {
        database,
        data_directory,
        definition,
        operation_id,
        request_json,
        result_json,
        document_defaults,
        routine,
        cancellation,
    } = commit;
    let mut connection = catalog::open_verified(database)?;
    if let Some(existing) = replay(&connection, commit)? {
        return Ok(existing);
    }
    check_cancellation(cancellation)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(catalog_error)?;
    // The write lock is held: a concurrent create with the same operation id
    // has either committed or not yet started, so this read is authoritative.
    if let Some(existing) = replay(&transaction, commit)? {
        return Ok(existing);
    }
    // A declared connection is only usable when its catalog is complete, so a
    // creation naming an unusable connection commits nothing.
    for connection_id in &definition.connections {
        crate::mcp::McpCatalogStore::require_complete_catalog(&transaction, connection_id)?;
    }
    store::insert(&transaction, definition)?;
    if let Some((routine_id, spec)) = routine {
        routines::store::insert_first_routine(
            &transaction,
            *routine_id,
            spec.clone(),
            definition.created_at_ms,
        )?;
    }
    store::insert_creation_receipt(
        &transaction,
        *operation_id,
        definition.id,
        request_json,
        result_json,
    )?;
    check_cancellation(cancellation)?;
    // Publication is the last step before the commit so that every rejection
    // above has no filesystem effect, while the row still cannot become visible
    // before its documents exist. A crash between the two publishes again on the
    // retry, and the identical files are adopted.
    if let Some((enabled, defaults)) = document_defaults {
        crate::documents::AgentDocumentStore::publish(
            data_directory,
            definition.id,
            *enabled,
            *defaults,
        )?;
    }
    transaction.commit().map_err(catalog_error)?;
    Ok(definition.clone())
}

/// Replays one creation, or reports a conflict when the stored operation used a
/// different actor or request.
fn replay(
    connection: &rusqlite::Connection,
    commit: &CreateCommit,
) -> Result<Option<AgentDefinition>, LocalHostError> {
    let Some(receipt) = store::creation_receipt(connection, commit.operation_id)? else {
        return Ok(None);
    };
    if !store::exists(connection, receipt.agent_id)? {
        return Err(LocalHostError::InvalidRequest(format!(
            "creation receipt for operation `{}` has no agent row",
            commit.operation_id
        )));
    }
    let stored: AgentDefinition = serde_json::from_str(&receipt.result_json)?;
    stored.validate()?;
    if stored.id != receipt.agent_id || stored.id != commit.definition.id {
        return Err(catalog::HostCatalogError::Invalid(format!(
            "creation receipt for operation `{}` describes agent {}, not `{}`",
            commit.operation_id, stored.id, receipt.agent_id
        ))
        .into());
    }
    if receipts_conflict(commit, &receipt, &stored) {
        return Err(LocalHostError::AgentConflict(receipt.agent_id));
    }
    Ok(Some(stored))
}

fn receipts_conflict(
    commit: &CreateCommit,
    receipt: &store::CreationReceipt,
    stored: &AgentDefinition,
) -> bool {
    receipt.request_json != commit.request_json
        || stored.creator != commit.definition.creator
        || stored.created_via != commit.definition.created_via
}

fn validate_actor(
    creator: &AgentCreator,
    origin: AgentCreationOrigin,
) -> Result<(), LocalHostError> {
    let trusted = matches!(
        (origin, creator),
        (AgentCreationOrigin::AgentTool, AgentCreator::Agent { .. })
            | (
                AgentCreationOrigin::Management,
                AgentCreator::Principal { .. }
            )
            | (
                AgentCreationOrigin::Provisioning,
                AgentCreator::System { .. }
            )
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
    baseline: &[capabilities::BuiltInCapability],
    documents: Option<AgentDocuments>,
    caller: &BTreeSet<String>,
) -> Result<BTreeSet<String>, LocalHostError> {
    let tools = capabilities::baseline_selection(baseline, caller);
    for name in &tools {
        require_selectable(name)?;
        require_consumable(name, documents)?;
    }
    Ok(tools)
}

fn data_directory(config: &HostConfig) -> Result<PathBuf, LocalHostError> {
    config
        .sessions
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| LocalHostError::InvalidRequest("Host sessions have no data root".to_owned()))
}

fn host_now_ms() -> Result<i64, LocalHostError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| LocalHostError::InvalidRequest(format!("Host clock error: {error}")))?;
    i64::try_from(elapsed.as_millis()).map_err(|_| {
        LocalHostError::InvalidRequest("Host clock exceeds the supported range".to_owned())
    })
}
