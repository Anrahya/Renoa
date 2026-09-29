//! The one canonical agent creation operation.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    AgentCreateRequest, LocalHost, LocalHostError, catalog_error, check_cancellation,
    require_selectable, store,
};
use crate::{
    AgentCreationOrigin, AgentCreator, AgentDefinition, AgentDocuments, AgentOperationalDefinition,
    AgentToolSelection, capabilities,
    host::{automations, catalog},
    presets,
    stable_id::stable_id,
};

// A frozen derivation domain: automation ids created by earlier releases were
// derived from it, so it keeps the name routines had then.
const AUTOMATION_ID_DOMAIN: &str = "renoa.agent.routine.v1";

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
        let database = self.config.database.clone();
        let operation = request.operation_id;
        let request_json = serde_json::to_string(&request)?;
        let saved_request = request_json.clone();
        let saved_creator = creator.clone();
        if let Some(definition) = tokio::task::spawn_blocking(move || {
            let db = catalog::open_verified(&database)?;
            replay(&db, operation, &saved_request, &saved_creator, origin)
        })
        .await??
        {
            return Ok(definition);
        }
        let preset = request
            .preset_id
            .as_ref()
            .map(presets::preset)
            .transpose()?;
        let documents = request
            .documents
            .or_else(|| preset.and_then(presets::AgentPreset::documents));
        if let Some(model) = &request.model {
            model.validate()?;
            let models =
                super::super::discover_models_for(&self.config, Some(model.provider)).await?;
            let selected =
                super::super::require_model(&models, model.provider, &model.model, "agent")?;
            super::super::initial_reasoning(selected, model.reasoning)?;
        }
        let definition = AgentDefinition {
            id: super::derived_agent_id(request.operation_id),
            name: request.name.clone(),
            created_at_ms: host_now_ms()?,
            creator,
            created_via: origin,
            preset_id: request.preset_id.clone(),
            operational: operational_definition(&request, preset, documents)?,
            tool_selection: AgentToolSelection {
                revision: 1,
                tools: resolve_selection(
                    preset.map_or(&[], presets::AgentPreset::capability_baseline),
                    request.tools.as_ref(),
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
        let result_json = serde_json::to_string(&definition)?;
        let automation = request.automation.map(|automation| {
            (
                stable_id(&format!("{AUTOMATION_ID_DOMAIN}:{}", request.operation_id)),
                automations::AutomationSpec {
                    agent_id: definition.id,
                    name: automation.name,
                    prompt: automation.prompt,
                    schedule: automation.schedule,
                    enabled: automation.enabled,
                },
            )
        });
        let commit = CreateCommit {
            database: self.config.database.clone(),
            data_directory: self.config.home.path().to_path_buf(),
            definition,
            operation_id: request.operation_id,
            request_json,
            result_json,
            documents: documents.map(|enabled| {
                (
                    enabled,
                    preset
                        .and_then(presets::AgentPreset::soul_default)
                        .unwrap_or_default(),
                )
            }),
            automation,
            cancellation,
        };
        tokio::task::spawn_blocking(move || create_blocking(&commit)).await?
    }
}

fn operational_definition(
    request: &AgentCreateRequest,
    preset: Option<&presets::AgentPreset>,
    documents: Option<AgentDocuments>,
) -> Result<AgentOperationalDefinition, LocalHostError> {
    Ok(AgentOperationalDefinition {
        instructions: match preset {
            Some(preset) => preset.instructions(request.instructions.as_deref()),
            None => request
                .instructions
                .clone()
                .ok_or(crate::AgentDefinitionError::EmptyInstructions)?,
        },
        behavior: request.behavior.unwrap_or_else(|| {
            preset.map_or(
                crate::AgentBehavior {
                    turn_timing: crate::TurnTiming::HostClock,
                    workspace_instructions: crate::WorkspaceInstructions::Off,
                    automatic_compaction: None,
                },
                presets::AgentPreset::behavior,
            )
        }),
        documents,
        provider_restriction: if request.model.is_some() {
            None
        } else {
            preset.and_then(presets::AgentPreset::provider_restriction)
        },
        model: request.model.clone(),
    })
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
    documents: Option<(AgentDocuments, &'static str)>,
    automation: Option<(Uuid, automations::AutomationSpec)>,
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
        documents,
        automation,
        cancellation,
    } = commit;
    let mut connection = catalog::open_verified(database)?;
    if let Some(existing) = replay(
        &connection,
        *operation_id,
        request_json,
        &definition.creator,
        definition.created_via,
    )? {
        return Ok(existing);
    }
    check_cancellation(cancellation)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(catalog_error)?;
    // The write lock is held: a concurrent create with the same operation id
    // has either committed or not yet started, so this read is authoritative.
    if let Some(existing) = replay(
        &transaction,
        *operation_id,
        request_json,
        &definition.creator,
        definition.created_via,
    )? {
        return Ok(existing);
    }
    // A declared connection is only usable when its catalog is complete, so a
    // creation naming an unusable connection commits nothing.
    for connection_id in &definition.connections {
        crate::mcp::McpCatalogStore::require_complete_catalog(&transaction, connection_id)?;
    }
    store::insert(&transaction, definition)?;
    if let Some((automation_id, spec)) = automation {
        automations::store::insert_first_automation(
            &transaction,
            *automation_id,
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
    if let Some((enabled, soul)) = documents {
        crate::documents::AgentDocumentStore::publish(
            data_directory,
            definition.id,
            *enabled,
            soul,
        )?;
    }
    transaction.commit().map_err(catalog_error)?;
    Ok(definition.clone())
}

/// Replays one creation, or reports a conflict when the stored operation used a
/// different actor or request.
fn replay(
    connection: &rusqlite::Connection,
    operation: Uuid,
    request_json: &str,
    creator: &AgentCreator,
    origin: AgentCreationOrigin,
) -> Result<Option<AgentDefinition>, LocalHostError> {
    let Some(receipt) = store::creation_receipt(connection, operation)? else {
        return Ok(None);
    };
    if !store::exists(connection, receipt.agent_id)? {
        return Err(LocalHostError::InvalidRequest(format!(
            "creation receipt for operation `{operation}` has no agent row"
        )));
    }
    let stored: AgentDefinition = serde_json::from_str(&receipt.result_json)?;
    stored.validate()?;
    if stored.id != receipt.agent_id || stored.id != super::derived_agent_id(operation) {
        return Err(catalog::HostCatalogError::Invalid(format!(
            "creation receipt for operation `{operation}` describes agent {}, not `{}`",
            stored.id, receipt.agent_id
        ))
        .into());
    }
    if receipt.request_json != request_json
        || stored.creator != *creator
        || stored.created_via != origin
    {
        return Err(LocalHostError::AgentConflict(receipt.agent_id));
    }
    Ok(Some(stored))
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
    caller: Option<&BTreeSet<String>>,
) -> Result<BTreeSet<String>, LocalHostError> {
    let tools = caller.cloned().unwrap_or_else(|| {
        baseline
            .iter()
            .map(|capability| capability.name().to_owned())
            .collect()
    });
    for name in &tools {
        require_selectable(name)?;
    }
    Ok(tools)
}

fn host_now_ms() -> Result<i64, LocalHostError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| LocalHostError::InvalidRequest(format!("Host clock error: {error}")))?;
    i64::try_from(elapsed.as_millis()).map_err(|_| {
        LocalHostError::InvalidRequest("Host clock exceeds the supported range".to_owned())
    })
}
